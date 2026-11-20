//! Wi-Fi 网络栈：AP / STA 管理、BIOS Setup Web UI、远程 shell 与 PXE 下载。

use core::{
    cell::RefCell,
    fmt::Write as _,
    net::{Ipv4Addr, SocketAddrV4},
    ptr,
    str::FromStr,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use alloc::{format, string::String, vec::Vec};
use critical_section::Mutex;
use embassy_executor::Spawner;
use embassy_net::{
    IpListenEndpoint, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4, dns::DnsQueryType,
    tcp::TcpSocket,
};
use embassy_time::Duration;
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config, ControllerConfig, Interface, WifiController,
    ap::AccessPointConfig, scan::ScanConfig, sta::StationConfig,
};
use static_cell::StaticCell;

use crate::{
    hal::{console, flash},
    nvram, shell,
};

/// 一条 HTTP 连接的空闲超时。浏览器会挂 keep-alive 不放。
const IDLE_TIMEOUT: Duration = Duration::from_secs(5);
/// 并发 HTTP 服务数。
/// TCP 收发缓冲区大小。
const TCP_BUF: usize = 2048;
/// 默认 AP 网关地址。
pub const AP_GATEWAY: &str = "192.168.4.1";
/// HTTP 监听端口。
pub const HTTP_PORT: u16 = 80;

/// 内嵌的 BIOS Setup 页面。
const INDEX_HTML: &str = include_str!("web/index.html");

// `Stack` 内含 `RefCell`，不是 `Sync`，所以不能放进 `Mutex`；
// 它由 `StaticCell` 永久持有，这里只存指针再借回来。
static STACK_PTR: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());
static STA_STACK_PTR: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());
static CONTROLLER_PTR: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());
static CONNECTED: AtomicBool = AtomicBool::new(false);
static WIFI_READY: AtomicBool = AtomicBool::new(false);
static WIFI_PTR: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());
/// 控制台输出环形缓冲，供远程 shell 拉取。
static CONSOLE_RING: Mutex<RefCell<ConsoleRing>> = Mutex::new(RefCell::new(ConsoleRing::new()));

/// 环形缓冲容量，必须是 2 的幂（用掩码取模）。
const CONSOLE_RING_SIZE: usize = 1024;

/// 控制台输出环形缓冲。
///
/// `head` 是**单调递增**的累计写入字节数，不是环形下标：`/c6wsh/stream` 的
/// `tail` 游标就是这个计数，前端靠它做增量续拉。回绕发生在 u32 上，
/// 4 GB 才转一圈，够用。
struct ConsoleRing {
    buf: [u8; CONSOLE_RING_SIZE],
    /// 累计写入字节数，单调递增。
    head: u32,
}

impl ConsoleRing {
    const fn new() -> Self {
        Self {
            buf: [0; CONSOLE_RING_SIZE],
            head: 0,
        }
    }

    fn push(&mut self, data: &[u8]) {
        for &b in data {
            self.buf[(self.head as usize) & (CONSOLE_RING_SIZE - 1)] = b;
            self.head = self.head.wrapping_add(1);
        }
    }

    /// 从 `tail` 游标读最多 `out.len()` 字节，返回 (实际字节数, 新游标)。
    ///
    /// 游标落在已滚出的区间时自动夹到最早可读位置，不会读出垃圾。
    fn read_from(&self, tail: u32, out: &mut [u8]) -> (usize, u32) {
        let head = self.head;
        let earliest = head.saturating_sub(CONSOLE_RING_SIZE as u32);
        let from = tail.clamp(earliest, head);
        let n = ((head - from) as usize).min(out.len());
        for (i, slot) in out[..n].iter_mut().enumerate() {
            *slot = self.buf[(from as usize + i) & (CONSOLE_RING_SIZE - 1)];
        }
        (n, from + n as u32)
    }
}

/// 控制台遥测抽头：把输出复制进环形缓冲。
pub fn console_tap(data: &[u8]) {
    critical_section::with(|cs| CONSOLE_RING.borrow_ref_mut(cs).push(data));
}

fn stack() -> Option<&'static Stack<'static>> {
    let p = STACK_PTR.load(Ordering::Acquire);
    if p.is_null() {
        None
    } else {
        // SAFETY: 指针指向 `StaticCell` 里永久存在的 `Stack`。
        Some(unsafe { &*(p as *const Stack<'static>) })
    }
}

fn sta_stack() -> Option<&'static Stack<'static>> {
    let p = STA_STACK_PTR.load(Ordering::Acquire);
    if p.is_null() {
        None
    } else {
        // SAFETY: 同 `stack`。
        Some(unsafe { &*(p as *const Stack<'static>) })
    }
}

fn controller() -> Option<&'static mut WifiController<'static>> {
    let p = CONTROLLER_PTR.load(Ordering::Acquire);
    if p.is_null() {
        None
    } else {
        // SAFETY: 同上；`WifiController` 只从这里取，调用点都是串行的。
        Some(unsafe { &mut *(p as *mut WifiController<'static>) })
    }
}

/// 交存 WIFI 外设，`init` 之后才能调。
pub fn set_peripheral(wifi: esp_hal::peripherals::WIFI<'static>) {
    static WIFI: StaticCell<esp_hal::peripherals::WIFI<'static>> = StaticCell::new();
    let stored = WIFI.init(wifi);
    WIFI_PTR.store(stored as *mut _ as *mut (), Ordering::Release);
    WIFI_READY.store(true, Ordering::Release);
}

/// 初始化 Wi-Fi 控制器与 embassy-net 协议栈。
///
/// AP 与 STA 共存：Setup 的 Web UI 必须随时能连，不能因为存了 STA 凭据就消失。
/// 因此 AP 侧（静态网关 + DHCP 服务端）永远建；有 SSID 时再叠一个 STA 接口。
/// 已初始化过时直接返回现有 AP 协议栈。
pub fn init(spawner: Spawner) -> Option<&'static Stack<'static>> {
    if let Some(stack) = stack() {
        return Some(stack);
    }
    if !WIFI_READY.load(Ordering::Acquire) {
        return None;
    }
    // SAFETY: `set_peripheral` 只写一次；`init` 也只会成功执行一次（上面已查过 `stack()`）。
    let wifi = unsafe {
        core::ptr::read(WIFI_PTR.load(Ordering::Acquire) as *const esp_hal::peripherals::WIFI<'static>)
    };

    let cfg = nvram::get();
    let has_sta = !cfg.ssid_str().is_empty();

    let initial = if has_sta {
        Config::AccessPointStation(
            sta_config(cfg.ssid_str(), cfg.pass_str()),
            ap_config(crate::BIOS_AP_SSID, crate::BIOS_AP_PASS),
        )
    } else {
        Config::AccessPoint(ap_config(crate::BIOS_AP_SSID, crate::BIOS_AP_PASS))
    };

    let mut controller = WifiController::new(
        wifi,
        ControllerConfig::default().with_initial_config(initial),
    )
    .ok()?;

    // 上行弱时认证帧会被 AP 忽略，表现为反复 AuthenticationExpired。
    // 默认发射功率偏保守，这里拉到最大。
    let _ = controller.set_max_tx_power(20);

    let gw = Ipv4Addr::from_str(AP_GATEWAY).unwrap_or(Ipv4Addr::new(192, 168, 4, 1));
    let rng = esp_hal::rng::Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    static AP_RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
    let (ap, ap_runner) = embassy_net::new(
        Interface::try_access_point()?,
        embassy_net::Config::ipv4_static(StaticConfigV4 {
            address: Ipv4Cidr::new(gw, 24),
            gateway: Some(gw),
            dns_servers: Default::default(),
        }),
        AP_RESOURCES.init(StackResources::new()),
        seed,
    );
    let ap: &'static Stack<'static> = {
        static S: StaticCell<Stack<'static>> = StaticCell::new();
        S.init(ap)
    };
    STACK_PTR.store(ap as *const _ as *mut (), Ordering::Release);

    if has_sta {
        spawn_sta(spawner, seed)?;
    }

    let controller: &'static mut WifiController<'static> = {
        static C: StaticCell<WifiController<'static>> = StaticCell::new();
        C.init(controller)
    };
    CONTROLLER_PTR.store(controller as *mut _ as *mut (), Ordering::Release);

    // 控制台输出无条件镜像进环形缓冲：Web Shell 的输出去靠它，
    // 不能只在 STA 连上时才装，否则 AP 模式下 `/c6wsh` 永远是空白。
    console::set_tap(Some(console_tap));

    spawner.spawn(net_task(ap_runner).unwrap());
    spawner.spawn(wifi_task().unwrap());
    spawner.spawn(http_task(ap).unwrap());
    spawner.spawn(dhcp_task(ap, gw).unwrap());

    Some(ap)
}

/// 建 STA 接口并挂上自带的 runner / HTTP 任务。
fn spawn_sta(spawner: Spawner, seed: u64) -> Option<&'static Stack<'static>> {
    static STA_RESOURCES: StaticCell<StackResources<6>> = StaticCell::new();
    let (sta, sta_runner) = embassy_net::new(
        Interface::try_station()?,
        embassy_net::Config::dhcpv4(Default::default()),
        STA_RESOURCES.init(StackResources::new()),
        seed ^ 1,
    );
    let sta: &'static Stack<'static> = {
        static S: StaticCell<Stack<'static>> = StaticCell::new();
        S.init(sta)
    };
    STA_STACK_PTR.store(sta as *const _ as *mut (), Ordering::Release);

    spawner.spawn(net_task_sta(sta_runner).unwrap());
    spawner.spawn(http_task_sta(sta).unwrap());
    Some(sta)
}

/// 补建 STA 接口：开机时没凭据就只建了 AP，`wifi connect` 之后得能补上。
pub fn ensure_sta(spawner: Spawner) -> bool {
    if sta_present() {
        return true;
    }
    let rng = esp_hal::rng::Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;
    spawn_sta(spawner, seed).is_some()
}

fn ap_config(ssid: &str, pass: &str) -> AccessPointConfig {
    let mut ssid_buf = [0u8; 32];
    let n = ssid.len().min(31);
    ssid_buf[..n].copy_from_slice(&ssid.as_bytes()[..n]);
    let mut pass_buf = [0u8; 64];
    let m = pass.len().min(63);
    pass_buf[..m].copy_from_slice(&pass.as_bytes()[..m]);

    AccessPointConfig::default()
        .with_ssid(ssid_buf[..n].try_into().unwrap_or_default())
        .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
            pass_buf[..m].try_into().unwrap_or_default(),
        ))
}

fn sta_config(ssid: &str, pass: &str) -> StationConfig {
    let mut ssid_buf = [0u8; 32];
    let n = ssid.len().min(31);
    ssid_buf[..n].copy_from_slice(&ssid.as_bytes()[..n]);
    let mut pass_buf = [0u8; 64];
    let m = pass.len().min(63);
    pass_buf[..m].copy_from_slice(&pass.as_bytes()[..m]);

    StationConfig::default()
        // 弱链路下关联经常超时，默认重试次数不够
        .with_failure_retry_cnt(10)
        .with_ssid(ssid_buf[..n].try_into().unwrap_or_default())
        .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
            pass_buf[..m].try_into().unwrap_or_default(),
        ))
}

/// 确保 AP 在广播（AP 常驻，`setup` 只是重新确认一次）。
pub fn start_ap() -> bool {
    let cfg = nvram::get();
    let ap = ap_config(crate::BIOS_AP_SSID, crate::BIOS_AP_PASS);
    let next = if cfg.ssid_str().is_empty() {
        Config::AccessPoint(ap)
    } else {
        Config::AccessPointStation(sta_config(cfg.ssid_str(), cfg.pass_str()), ap)
    };
    controller().is_some_and(|c| c.set_config(&next).is_ok())
}

/// 带上 STA 重新下发配置，`wifi connect` 与开机自动联网都走这里。
pub fn start_sta() -> bool {
    let cfg = nvram::get();
    if cfg.ssid_str().is_empty() {
        return false;
    }
    let next = Config::AccessPointStation(
        sta_config(cfg.ssid_str(), cfg.pass_str()),
        ap_config(crate::BIOS_AP_SSID, crate::BIOS_AP_PASS),
    );
    controller().is_some_and(|c| c.set_config(&next).is_ok())
}

/// 只关 STA、保留 AP：`wifi disconnect` 清掉凭据后还要能从 AP 重新配网。
pub fn stop_ap() {
    let ap = ap_config(crate::BIOS_AP_SSID, crate::BIOS_AP_PASS);
    let _ = controller().is_some_and(|c| c.set_config(&Config::AccessPoint(ap)).is_ok());
}

/// STA 是否已经拿到 IP。
pub fn is_connected() -> bool {
    CONNECTED.load(Ordering::Acquire)
}

/// 把当前 IPv4 地址写进 `buf`，成功返回 0。
pub fn copy_ip(buf: Option<&mut [u8]>) -> i32 {
    let Some(buf) = buf else { return -1 };
    let Some(stack) = stack() else { return -1 };
    let Some(cfg) = stack.config_v4() else {
        return -1;
    };
    let mut s = String::new();
    let _ = write!(s, "{}", cfg.address.address());
    let n = s.len().min(buf.len().saturating_sub(1));
    buf[..n].copy_from_slice(&s.as_bytes()[..n]);
    buf[n] = 0;
    0
}

/// 当前 IPv4 地址字符串。
pub fn ip_string() -> String {
    stack()
        .and_then(|s| s.config_v4())
        .map(|c| alloc::format!("{}", c.address.address()))
        .unwrap_or_else(|| "0.0.0.0".into())
}

/// 协议栈是否已经初始化过。
pub fn stack_ptr_is_null() -> bool {
    STACK_PTR.load(Ordering::Acquire).is_null()
}

/// 是否配置了 STA 接口（NVRAM 里有凭据）。
pub fn sta_present() -> bool {
    !STA_STACK_PTR.load(Ordering::Acquire).is_null()
}

/// STA 侧拿到的地址；未连接时返回 `-`。
pub fn sta_ip_string() -> String {
    sta_stack()
        .and_then(|s| s.config_v4())
        .map(|c| alloc::format!("{}", c.address.address()))
        .unwrap_or_else(|| "-".into())
}

/// 扫描周边 AP 并打印结果。
///
/// `scan_async` 是异步的，只能从异步的 `repl` 里调；同步的 `run()` 分发不到这里。
/// AP-only 模式下驱动不支持扫描，提前挡掉给个明确提示。
pub async fn scan_print() {
    if !sta_present() {
        console::print("wifi scan: 只有 AP 接口时驱动不支持扫描，先 wifi connect 配好 STA\r\n");
        return;
    }
    let Some(ctrl) = controller() else {
        console::print("wifi scan: 先 setup 或 wifi connect 初始化 Wi-Fi\r\n");
        return;
    };

    console::print("扫描中...\r\n");
    let cfg = ScanConfig::default().with_max(16);
    match ctrl.scan_async(&cfg).await {
        Ok(list) => {
            console::print("SSID                             RSSI  CH\r\n");
            console::print("-------------------------------------------\r\n");
            for ap in &list {
                console::print(&format!(
                    "{:<32} {:>4}  {:>2}\r\n",
                    ap.ssid.as_str(),
                    ap.signal_strength,
                    ap.channel
                ));
            }
            console::print(&format!("共 {} 个\r\n", list.len()));
        }
        Err(_) => console::print("wifi scan 失败\r\n"),
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await;
}

#[embassy_executor::task]
async fn net_task_sta(mut runner: Runner<'static, Interface>) {
    runner.run().await;
}

/// 给连上 AP 的设备分配地址。没有它客户端关联成功也会报「IP 配置失败」。
#[embassy_executor::task]
async fn dhcp_task(stack: &'static Stack<'static>, gw: Ipv4Addr) {
    use edge_dhcp::{
        io::{self, DEFAULT_SERVER_PORT},
        server::{Server, ServerOptions},
    };
    use edge_nal::UdpBind;
    use edge_nal_embassy::{Udp, UdpBuffers};

    let mut buf = [0u8; 1500];
    let mut gw_buf = [Ipv4Addr::UNSPECIFIED];

    let buffers = UdpBuffers::<3, 1024, 1024, 10>::new();
    let unbound = Udp::new(*stack, &buffers);
    let Ok(mut socket) = unbound
        .bind(core::net::SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            DEFAULT_SERVER_PORT,
        )))
        .await
    else {
        console::print("[NET] DHCP 服务端绑定失败，客户端将拿不到地址\r\n");
        return;
    };
    console::print("[NET] DHCP 服务端已启动\r\n");

    loop {
        let _ = io::server::run(
            &mut Server::<_, 64>::new_with_et(gw),
            &ServerOptions::new(gw, Some(&mut gw_buf)),
            &mut socket,
            &mut buf,
        )
        .await;
        embassy_time::Timer::after(Duration::from_millis(500)).await;
    }
}

#[embassy_executor::task]
async fn wifi_task() {
    loop {
        if sta_present() && !CONNECTED.load(Ordering::Acquire) {
            if let Some(c) = controller() {
                match c.connect_async().await {
                    Ok(_) => defmt::info!("[NET] 关联成功"),
                    Err(e) => {
                        defmt::info!("[NET] 关联失败: {}", defmt::Debug2Format(&e));
                        embassy_time::Timer::after(Duration::from_secs(3)).await;
                    }
                }
            }
        }
        let up = controller().is_some_and(|c| c.is_connected());
        CONNECTED.store(up, Ordering::Release);
        embassy_time::Timer::after(Duration::from_millis(500)).await;
    }
}

/// HTTP 主循环：串行处理一条连接，够 BIOS Setup 这种低频交互用。
#[embassy_executor::task]
async fn http_task(stack: &'static Stack<'static>) {
    serve_http(stack).await;
}

/// STA 侧的同一套页面：连上路由器后也能用板子拿到的局域网地址访问。
#[embassy_executor::task]
async fn http_task_sta(stack: &'static Stack<'static>) {
    serve_http(stack).await;
}

async fn serve_http(stack: &'static Stack<'static>) -> ! {
    let mut rx_buf = [0u8; TCP_BUF];
    let mut tx_buf = [0u8; TCP_BUF];

    loop {
        let mut socket = TcpSocket::new(*stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(IDLE_TIMEOUT));

        if socket
            .accept(IpListenEndpoint {
                addr: None,
                port: HTTP_PORT,
            })
            .await
            .is_err()
        {
            continue;
        }

        let mut request = [0u8; TCP_BUF];
        let mut len = 0usize;
        let mut header_end = None;
        loop {
            match socket.read(&mut request[len..]).await {
                Ok(0) => break,
                Ok(n) => {
                    len += n;
                    if let Some(p) = request[..len].windows(4).position(|w| w == b"\r\n\r\n") {
                        header_end = Some(p + 4);
                        break;
                    }
                    if len == request.len() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        // 请求体必须读干净再关连接：内核发现接收缓冲还有未读数据会发 RST，
        // 浏览器侧 `fetch` 直接抛异常，前端只能报「Command dispatch failed」。
        if let Some(he) = header_end {
            let want = (he + content_length(&request[..he])).min(request.len());
            while len < want {
                match socket.read(&mut request[len..want]).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => len += n,
                }
            }
        }

        let response = handle_request(&request[..len]).await;
        // tx 缓冲只有 TCP_BUF，一次 `write` 只吃得下这么多，剩下的必须续写。
        // 否则大页面（`/c6wsh` 4.2 KB、`/` 15.6 KB）被截断，浏览器卡在等剩下的字节。
        let body = response.as_bytes();
        let mut sent = 0;
        while sent < body.len() {
            match socket.write(&body[sent..]).await {
                Ok(0) | Err(_) => break,
                Ok(n) => sent += n,
            }
        }
        let _ = socket.flush().await;
        socket.close();

        // 重启请求要等响应真正发出去再动手，否则浏览器只看到连接被断开
        if REBOOT_PENDING.load(Ordering::Acquire) {
            crate::hal::console::print("[WEB] 重启中...\r\n");
            embassy_time::Timer::after(Duration::from_millis(200)).await;
            crate::hal::power::reboot();
        }

        embassy_time::Timer::after(Duration::from_millis(10)).await;
    }
}

/// Web UI 请求重启后，等响应发完再真正重启。
static REBOOT_PENDING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Web Shell 页面，来自上游 `web_ui/c6wsh.c` 的嵌入式 HTML。
const C6WSH_HTML: &str = include_str!("web/c6wsh.html");

/// 从请求头里取 `Content-Length`，没有就按 0 算。
fn content_length(head: &[u8]) -> usize {
    let text = String::from_utf8_lossy(head);
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            return v.trim().parse().unwrap_or(0);
        }
    }
    0
}

/// 路由一条 HTTP 请求。
async fn handle_request(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut lines = text.lines();
    let Some(request_line) = lines.next() else {
        return http_error(400, "BAD REQUEST");
    };

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");

    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .unwrap_or("");

    match (method, path) {
        ("GET", "/") => http_ok("text/html; charset=utf-8", INDEX_HTML),
        ("GET", "/api/settings") => http_ok("application/json", &settings_json()),
        ("POST", "/api/settings") => {
            apply_settings(body);
            http_ok("application/json", "{\"ok\":true}")
        }
        ("GET", "/api/sysinfo") => http_ok("application/json", &sysinfo_json()),
        ("POST", "/api/reboot") => {
            defmt::warn!("[WEB] 收到重启请求");
            REBOOT_PENDING.store(true, Ordering::Release);
            http_ok("application/json", "{\"ok\":true}")
        }
        ("POST", "/api/defaults") => {
            nvram::load_defaults();
            http_ok("application/json", "{\"ok\":true}")
        }
        ("POST", "/api/trigger_ota") => {
            nvram::update(|c| c.bios_update_state = nvram::BiosUpdateState::Pending);
            http_ok("application/json", "{\"ok\":true}")
        }
        ("GET", "/c6wsh") => http_ok("text/html; charset=utf-8", C6WSH_HTML),
        ("GET", "/c6wsh/stream") => {
            let head = critical_section::with(|cs| CONSOLE_RING.borrow_ref(cs).head);
            // 带了 tail 就按游标续拉；缺省或落后超过整圈时退回最近 512 字节
            let tail = match query_param(target, "tail").and_then(|s| s.parse::<u32>().ok()) {
                Some(t) if head.saturating_sub(t) <= CONSOLE_RING_SIZE as u32 => t,
                _ => head.saturating_sub(512),
            };
            let mut chunk = [0u8; 512];
            let (n, new_tail) =
                critical_section::with(|cs| CONSOLE_RING.borrow_ref(cs).read_from(tail, &mut chunk));
            let body = String::from_utf8_lossy(&chunk[..n]).into_owned();
            http_ok_hdr(
                "text/plain; charset=utf-8",
                &alloc::format!("X-Tail: {new_tail}\r\n"),
                &body,
            )
        }
        ("POST", "/c6wsh/exec") => {
            let cmd = body.trim();
            if cmd.is_empty() {
                http_error(400, "BAD REQUEST")
            } else {
                // 命令输出直接进环形缓冲，前端靠 /c6wsh/stream 增量取回
                let _ = shell::execute_capture(cmd);
                http_ok("text/plain; charset=utf-8", "OK")
            }
        }
        ("GET", "/wsh") => {
            // 先取走积压的控制台输出，再执行命令并附上它的回显
            let mut chunk = [0u8; 512];
            let head = critical_section::with(|cs| CONSOLE_RING.borrow_ref(cs).head);
            let (n, _) = critical_section::with(|cs| {
                CONSOLE_RING
                    .borrow_ref(cs)
                    .read_from(head.saturating_sub(512), &mut chunk)
            });
            let pending = String::from_utf8_lossy(&chunk[..n]).into_owned();
            let cmd = query_param(target, "cmd").unwrap_or_default();
            let output = if cmd.trim().is_empty() {
                pending
            } else {
                alloc::format!("{pending}{}", shell::execute_capture(&cmd))
            };
            http_ok("text/plain; charset=utf-8", &output)
        }
        _ => http_error(404, "NOT FOUND"),
    }
}

/// 从 URL 查询串里取一个参数。
fn query_param(target: &str, key: &str) -> Option<String> {
    let (_, query) = target.split_once('?')?;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == key {
            return Some(percent_decode(v));
        }
    }
    None
}

/// 最小 percent-decode，只处理 `%XX` 与 `+`。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = core::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn http_ok(content_type: &str, body: &str) -> String {
    alloc::format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    )
}

/// 带自定义响应头的 200 响应。
fn http_ok_hdr(content_type: &str, extra: &str, body: &str) -> String {
    alloc::format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n{extra}Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    )
}

fn http_error(code: u16, msg: &str) -> String {
    alloc::format!(
        "HTTP/1.1 {code} ERROR\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{msg}",
        msg.len()
    )
}

fn settings_json() -> String {
    let c = nvram::get();
    alloc::format!(
        "{{\"dc_loss\":{},\"post_led\":{},\"c6wsh\":{},\"wifi_ssid\":\"{}\",\"wifi_pass\":\"{}\",\"wifi_country\":\"{}\",\"pxe_url\":\"{}\",\"aura_mode\":{},\"aura_brightness\":{},\"cpu_freq\":{},\"cpu_gov\":{},\"bod_lvl\":{},\"temp_th\":{},\"temp_em\":{},\"zc6_swap\":{}}}",
        matches!(c.dc_loss_action, nvram::DcLossAction::PowerOn) as u8,
        matches!(c.post_led_mode, nvram::PostLedMode::Enabled) as u8,
        matches!(c.c6wsh_state, nvram::C6wshState::Enabled) as u8,
        c.ssid_str(),
        c.pass_str(),
        c.country_str(),
        c.pxe_url_str(),
        match c.aura_mode {
            nvram::AuraMode::Static => 0,
            nvram::AuraMode::Rainbow => 1,
            nvram::AuraMode::Disabled => 2,
        },
        c.aura_brightness,
        c.cpu_freq.mhz(),
        matches!(c.cpu_governor, nvram::CpuGovernor::Dynamic) as u8,
        match c.bod_level {
            nvram::BodLevel::Strict => 0,
            nvram::BodLevel::Relaxed => 1,
            nvram::BodLevel::Disabled => 2,
        },
        c.thermal_throttle_c,
        c.thermal_emergency_c,
        match c.zc6_swap_mode {
            nvram::Zc6SwapMode::Disabled => 0,
            nvram::Zc6SwapMode::Flash => 1,
            nvram::Zc6SwapMode::Ram => 2,
            nvram::Zc6SwapMode::Both => 3,
        },
    )
}

fn sysinfo_json() -> String {
    let parts = partition_json();
    alloc::format!(
        "{{\"cpu_real_mhz\":{},\"cpu_temp_c\":{},\"me_active\":{},\"nvs_used_bytes\":{},\"nvs_free_bytes\":{},\"ram_total_kb\":{},\"ram_free_kb\":{},\"flash_total_mb\":{},\"partitions\":[{}]}}",
        crate::hal::clock::get_cpu_mhz(),
        crate::me::temperature_c(),
        crate::me::is_active() as u8,
        nvs_used(),
        nvs_total() - nvs_used(),
        (esp_alloc::HEAP.used() + esp_alloc::HEAP.free()) / 1024,
        esp_alloc::HEAP.free() / 1024,
        flash::chip_size() / (1024 * 1024),
        parts,
    )
}

/// 分区表 JSON 片段。
fn partition_json() -> String {
    let mut out = String::new();
    for (name, p) in flash::all_partitions() {
        if p.size == 0 {
            continue;
        }
        if !out.is_empty() {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"name\":\"{}\",\"offset\":{},\"size_kb\":{}}}",
            name,
            p.offset,
            p.size / 1024
        );
    }
    out
}

/// NVRAM 已用条目数（固定布局只有 1 块）。
fn nvs_used() -> u32 {
    nvram::CONFIG_BYTES as u32
}

/// NVRAM 分区总容量（字节）。
fn nvs_total() -> u32 {
    flash::nvs_partition().map_or(0, |p| p.size)
}

/// 解析并落盘 Web UI 提交的设置。
fn apply_settings(body: &str) {
    nvram::update(|c| {
        if let Some(v) = json_u32(body, "dc_loss") {
            c.dc_loss_action = if v == 1 {
                nvram::DcLossAction::PowerOn
            } else {
                nvram::DcLossAction::PowerOff
            };
        }
        if let Some(v) = json_u32(body, "post_led") {
            c.post_led_mode = if v == 1 {
                nvram::PostLedMode::Enabled
            } else {
                nvram::PostLedMode::Disabled
            };
        }
        if let Some(v) = json_u32(body, "c6wsh") {
            c.c6wsh_state = if v == 1 {
                nvram::C6wshState::Enabled
            } else {
                nvram::C6wshState::Disabled
            };
        }
        if let Some(v) = json_u32(body, "aura_mode") {
            c.aura_mode = match v {
                0 => nvram::AuraMode::Static,
                2 => nvram::AuraMode::Disabled,
                _ => nvram::AuraMode::Rainbow,
            };
        }
        if let Some(v) = json_u32(body, "aura_brightness") {
            c.aura_brightness = v as u8;
        }
        if let Some(v) = json_u32(body, "cpu_freq") {
            c.cpu_freq = match v {
                80 => nvram::CpuFreq::Mhz80,
                120 => nvram::CpuFreq::Mhz120,
                _ => nvram::CpuFreq::Mhz160,
            };
        }
        if let Some(v) = json_u32(body, "cpu_gov") {
            c.cpu_governor = if v == 1 {
                nvram::CpuGovernor::Dynamic
            } else {
                nvram::CpuGovernor::Performance
            };
        }
        if let Some(v) = json_u32(body, "bod_lvl") {
            c.bod_level = match v {
                1 => nvram::BodLevel::Relaxed,
                2 => nvram::BodLevel::Disabled,
                _ => nvram::BodLevel::Strict,
            };
        }
        if let Some(v) = json_u32(body, "zc6_swap") {
            c.zc6_swap_mode = match v {
                0 => nvram::Zc6SwapMode::Disabled,
                1 => nvram::Zc6SwapMode::Flash,
                2 => nvram::Zc6SwapMode::Ram,
                _ => nvram::Zc6SwapMode::Both,
            };
        }
        if let Some(v) = json_u32(body, "temp_th") {
            c.thermal_throttle_c = v as u8;
        }
        if let Some(v) = json_u32(body, "temp_em") {
            c.thermal_emergency_c = v as u8;
        }
        if let Some(v) = json_str(body, "wifi_ssid") {
            let pass = json_str(body, "wifi_pass").unwrap_or_default();
            c.set_wifi_sta(&v, &pass);
        }
        if let Some(v) = json_str(body, "wifi_country") {
            c.set_wifi_country(&v);
        }
        if let Some(v) = json_str(body, "pxe_url") {
            c.set_pxe_url(&v);
        }
    });
}

/// 从扁平 JSON 里取一个整数字段。
fn json_u32(body: &str, key: &str) -> Option<u32> {
    let needle = alloc::format!("\"{key}\"");
    let start = body.find(&needle)? + needle.len();
    let rest = &body[start..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// 从扁平 JSON 里取一个字符串字段。
fn json_str(body: &str, key: &str) -> Option<String> {
    let needle = alloc::format!("\"{key}\"");
    let start = body.find(&needle)? + needle.len();
    let rest = &body[start..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].into())
}

/// 通过 HTTP 把一个 URL 的内容下载进内存，用于 PXE 与 OTA。
///
/// shortcut: 只支持明文 HTTP + `Content-Length`，不支持 chunked 和重定向；换 HTTPS 源时重写。
pub async fn http_download(url: &str) -> Option<Vec<u8>> {
    let stack = sta_stack()?;
    let (host, port, path) = parse_url(url)?;

    // 字面量 IP 直接解析，别丢给 DNS——DNS 会被拖到超时才失败
    let ip = match host.parse::<Ipv4Addr>() {
        Ok(v4) => v4,
        Err(_) => stack
            .dns_query(host.as_str(), DnsQueryType::A)
            .await
            .ok()
            .and_then(|v| v.first().copied())
            .and_then(|a| match a {
                embassy_net::IpAddress::Ipv4(v4) => Some(v4),
            })?,
    };

    let mut rx_buf = [0u8; TCP_BUF];
    let mut tx_buf = [0u8; TCP_BUF];
    let mut socket = TcpSocket::new(*stack, &mut rx_buf, &mut tx_buf);
    socket.set_timeout(Some(Duration::from_secs(30)));

    socket
        .connect(SocketAddrV4::new(ip, port))
        .await
        .ok()?;

    let request = alloc::format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    );
    socket.write(request.as_bytes()).await.ok()?;
    socket.flush().await.ok()?;

    let mut raw = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match socket.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    socket.close();

    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    Some(raw[split + 4..].to_vec())
}

/// 拆 `http://host:port/path`。
fn parse_url(url: &str) -> Option<(String, u16, String)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, p.parse().unwrap_or(80)),
        None => (authority, 80),
    };
    Some((host.into(), port, path.into()))
}

/// payload 侧的 BSD socket 系统调用。
///
/// 上游靠 lwIP 的阻塞 socket 实现；embassy-net 是异步的，在 trap 上下文里没法驱动
/// 执行器，所以这里返回 -1，payload 的网络访问改走 `pxe`/`wsh` 这类内核侧接口。
pub fn tcp_syscall(_nr: u32, _args: &[u32]) -> i32 {
    -1
}

