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
static CONTROLLER_PTR: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());
static CONNECTED: AtomicBool = AtomicBool::new(false);
static AP_MODE: AtomicBool = AtomicBool::new(false);
static WIFI_READY: AtomicBool = AtomicBool::new(false);
static WIFI_PTR: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());
/// 控制台输出环形缓冲，供远程 shell 拉取。
static CONSOLE_RING: Mutex<RefCell<ConsoleRing>> = Mutex::new(RefCell::new(ConsoleRing::new()));

/// 控制台输出环形缓冲，1 KB 足够一次命令的回显。
struct ConsoleRing {
    buf: [u8; 1024],
    len: usize,
}

impl ConsoleRing {
    const fn new() -> Self {
        Self {
            buf: [0; 1024],
            len: 0,
        }
    }

    fn push(&mut self, data: &[u8]) {
        for &b in data {
            if self.len == self.buf.len() {
                self.buf.copy_within(1.., 0);
                self.len -= 1;
            }
            self.buf[self.len] = b;
            self.len += 1;
        }
    }

    fn take(&mut self) -> String {
        let s = String::from_utf8_lossy(&self.buf[..self.len]).into_owned();
        self.len = 0;
        s
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
/// 初始配置按 NVRAM 决定：有 SSID 就起 STA，否则起 AP Setup 模式。
/// 已初始化过时直接返回现有协议栈。
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

    let initial = if cfg.ssid_str().is_empty() {
        ap_config("BIOS_SETUP_C6", "12345678")
    } else {
        sta_config(cfg.ssid_str(), cfg.pass_str())
    };
    let ap_mode = cfg.ssid_str().is_empty();

    let mut controller = WifiController::new(
        wifi,
        ControllerConfig::default().with_initial_config(initial),
    )
    .ok()?;

    // 上行弱时认证帧会被 AP 忽略，表现为反复 AuthenticationExpired。
    // 默认发射功率偏保守，这里拉到最大。
    let _ = controller.set_max_tx_power(20);

    let device = if ap_mode {
        Interface::access_point()
    } else {
        Interface::station()
    };

    let net_config = if ap_mode {
        let gw = Ipv4Addr::from_str(AP_GATEWAY).unwrap_or(Ipv4Addr::new(192, 168, 4, 1));
        embassy_net::Config::ipv4_static(StaticConfigV4 {
            address: Ipv4Cidr::new(gw, 24),
            gateway: Some(gw),
            dns_servers: Default::default(),
        })
    } else {
        embassy_net::Config::dhcpv4(Default::default())
    };

    static RESOURCES: StaticCell<StackResources<6>> = StaticCell::new();
    let rng = esp_hal::rng::Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;
    let (stack, runner) = embassy_net::new(
        device,
        net_config,
        RESOURCES.init(StackResources::new()),
        seed,
    );

    let stack: &'static Stack<'static> = {
        static S: StaticCell<Stack<'static>> = StaticCell::new();
        S.init(stack)
    };

    let controller: &'static mut WifiController<'static> = {
        static C: StaticCell<WifiController<'static>> = StaticCell::new();
        C.init(controller)
    };

    STACK_PTR.store(stack as *const _ as *mut (), Ordering::Release);
    CONTROLLER_PTR.store(controller as *mut _ as *mut (), Ordering::Release);
    AP_MODE.store(ap_mode, Ordering::Release);

    spawner.spawn(net_task(runner).unwrap());
    spawner.spawn(wifi_task().unwrap());
    spawner.spawn(http_task(stack).unwrap());

    Some(stack)
}

fn ap_config(ssid: &str, pass: &str) -> Config {
    let mut ssid_buf = [0u8; 32];
    let n = ssid.len().min(31);
    ssid_buf[..n].copy_from_slice(&ssid.as_bytes()[..n]);
    let mut pass_buf = [0u8; 64];
    let m = pass.len().min(63);
    pass_buf[..m].copy_from_slice(&pass.as_bytes()[..m]);

    Config::AccessPoint(
        AccessPointConfig::default()
            .with_ssid(ssid_buf[..n].try_into().unwrap_or_default())
            .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
                pass_buf[..m].try_into().unwrap_or_default(),
            )),
    )
}

fn sta_config(ssid: &str, pass: &str) -> Config {
    let mut ssid_buf = [0u8; 32];
    let n = ssid.len().min(31);
    ssid_buf[..n].copy_from_slice(&ssid.as_bytes()[..n]);
    let mut pass_buf = [0u8; 64];
    let m = pass.len().min(63);
    pass_buf[..m].copy_from_slice(&pass.as_bytes()[..m]);

    Config::Station(
        StationConfig::default()
            // 弱链路下关联经常超时，默认重试次数不够
            .with_failure_retry_cnt(10)
            .with_ssid(ssid_buf[..n].try_into().unwrap_or_default())
            .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
                pass_buf[..m].try_into().unwrap_or_default(),
            )),
    )
}

/// 切到 AP 模式。
pub fn start_ap(ssid: &str, pass: &str) -> bool {
    let cfg = ap_config(ssid, pass);
    let ok = controller().is_some_and(|c| c.set_config(&cfg).is_ok());
    if ok {
        AP_MODE.store(true, Ordering::Release);
    }
    ok
}

/// 切到 STA 模式并用 NVRAM 里的凭据连接。
pub fn start_sta() -> bool {
    let cfg = nvram::get();
    if cfg.ssid_str().is_empty() {
        return false;
    }
    let sta = sta_config(cfg.ssid_str(), cfg.pass_str());
    let ok = controller().is_some_and(|c| c.set_config(&sta).is_ok());
    if ok {
        AP_MODE.store(false, Ordering::Release);
    }
    ok
}

/// 退出 AP 模式，回到未连接的 STA。
///
/// 驱动不允许 AP-only 模式下扫描，所以 `wifi disconnect` 必须真的把模式切回去，
/// 否则 `wifi scan` 会一直卡在「AP 模式不支持扫描」。
pub fn stop_ap() {
    let switched = controller()
        .is_some_and(|c| c.set_config(&Config::Station(StationConfig::default())).is_ok());
    if switched {
        AP_MODE.store(false, Ordering::Release);
    }
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

/// 当前模式是否为 AP。
/// 协议栈是否已经初始化过。
pub fn stack_ptr_is_null() -> bool {
    STACK_PTR.load(Ordering::Acquire).is_null()
}

pub fn is_ap() -> bool {
    AP_MODE.load(Ordering::Acquire)
}

/// 扫描周边 AP 并打印结果。
///
/// `scan_async` 是异步的，只能从异步的 `repl` 里调；同步的 `run()` 分发不到这里。
/// AP-only 模式下驱动不支持扫描，提前挡掉给个明确提示。
pub async fn scan_print() {
    if is_ap() {
        console::print("wifi scan: AP 模式下不支持扫描，先 wifi disconnect\r\n");
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
async fn wifi_task() {
    loop {
        if !is_ap() && !CONNECTED.load(Ordering::Acquire) {
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

        let mut request = [0u8; 1024];
        let mut len = 0usize;
        loop {
            match socket.read(&mut request[len..]).await {
                Ok(0) => break,
                Ok(n) => {
                    len += n;
                    if request[..len].windows(4).any(|w| w == b"\r\n\r\n") || len == request.len() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        let response = handle_request(&request[..len]).await;
        let _ = socket.write(response.as_bytes()).await;
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
        ("GET", "/wsh") => {
            // 先取走积压的控制台输出，再执行命令并附上它的回显
            let pending = critical_section::with(|cs| CONSOLE_RING.borrow_ref_mut(cs).take());
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
        "{{\"cpu_real_mhz\":{},\"cpu_temp_c\":{},\"nvs_used_entries\":{},\"nvs_free_entries\":{},\"ram_total_kb\":{},\"ram_free_kb\":{},\"flash_total_mb\":{},\"partitions\":[{}]}}",
        crate::hal::clock::get_cpu_mhz(),
        crate::me::temperature_c(),
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
    for (name, part) in [
        ("openc6_fs", flash::fs_partition()),
        ("nvs", flash::nvs_partition()),
    ] {
        if let Some(p) = part {
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
    }
    out
}

/// NVRAM 已用条目数（固定布局只有 1 块）。
fn nvs_used() -> u32 {
    1
}

/// NVRAM 总条目数。
fn nvs_total() -> u32 {
    1
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
    let stack = stack()?;
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

