//! WiFi：AP 常驻 + STA netif（扫描要它才工作，openc6 实测结论）
//! + DHCP + DNS 劫持 + captive portal HTTP + 扫描管理任务。
//! 对应上游 `wifi_ap_manager` / `dns_server` / captive portal / wardriving 组件。
//! portal 表单提交 STA 凭据存进 settings（NV）；esp-radio 无法干净去初始化，
//! 网络栈启动后常驻到重启。

use alloc::{string::String, vec::Vec};
use core::fmt::Write as _;
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use core::str::FromStr;
use core::sync::atomic::{AtomicBool, Ordering};

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::{
    IpListenEndpoint, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4, tcp::TcpSocket,
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    signal::Signal,
};
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::rng::Rng;
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config, ControllerConfig, Interface, WifiController,
    ap::AccessPointConfig,
    scan::{ScanConfig, ScanTypeConfig},
    sta::StationConfig,
};
use static_cell::StaticCell;

use crate::{radio, settings};

/// 热点 SSID
pub const SSID: &str = "minio-rs";
/// 热点密码，WPA2-Personal
pub const PASSWORD: &str = "12345678";
/// 板子地址，同时是 portal 地址与 DNS 劫持目标
pub const GATEWAY: &str = "192.168.2.1";
/// HTTP 监听端口
const PORT: u16 = 80;
/// 并发连接数：每实例各占一套收发缓冲，浏览器会同时开好几条
const SERVERS: usize = 3;
/// 一条连接的空闲超时，短一点让 keep-alive 挂着的连接让位
const IDLE_TIMEOUT: Duration = Duration::from_secs(5);
/// POST 请求体最大长度
const BODY_MAX: usize = 256;
/// 扫描结果上限
const SCAN_MAX: usize = 16;
/// 主动扫描每信道驻留：默认 10–20 ms 弱天线会漏掉大半 AP（openc6 实测）
const SCAN_DWELL_MIN_MS: u64 = 80;
const SCAN_DWELL_MAX_MS: u64 = 120;
/// 单次 STA 关联超时：`connect_async` 自身无超时，弱信号下实测能卡 80 s 不返回
const STA_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// 关联失败后的退避间隔
const STA_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// WiFi 栈是否已启动
pub static WIFI_UP: AtomicBool = AtomicBool::new(false);

/// 扫描请求：wardriving app 置位，manager 任务消费
static SCAN_REQUEST: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// STA 掉线重连请求：sta_dhcp 检测到链路丢失时置位，manager 执行重连
static RECONNECT_REQUEST: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// portal 首页：STA 凭据录入表单
const PORTAL_HTML: &str = concat!(
    "<!doctype html><html><head><meta charset=\"utf-8\">",
    "<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">",
    "<title>minio-rs</title></head><body>",
    "<h1>minio-rs captive portal</h1>",
    "<p>Store WiFi STA credentials on the device</p>",
    "<form method=\"post\" action=\"/save\">",
    "<label>SSID <input name=\"ssid\"></label><br>",
    "<label>Pass <input name=\"pass\" type=\"password\"></label><br>",
    "<button>Save</button></form></body></html>",
);

/// 请求 WiFi 栈启动（portal / wardriving 共用）；重复调用无害
pub fn start(spawner: &Spawner, wifi: esp_hal::peripherals::WIFI<'static>) -> Result<(), &'static str> {
    if WIFI_UP.load(Ordering::Relaxed) {
        return Ok(());
    }
    // 射频租约：WiFi 独占
    if let Err(e) = radio::acquire(minio_rs_core::lease::RadioKind::Wifi) {
        return Err(match e {
            minio_rs_core::lease::LeaseError::Busy(_) => "radio busy",
            _ => "unsupported",
        });
    }

    let gw = Ipv4Addr::from_str(GATEWAY).map_err(|_| "bad gateway ip")?;
    let ap_config = AccessPointConfig::default()
        .with_ssid(SSID.try_into().map_err(|_| "ssid too long")?)
        .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
            PASSWORD.try_into().map_err(|_| "pass too long")?,
        ));
    // portal 存下的 STA 凭据：有就开 AP+STA 共存模式，没有就纯 AP
    let creds = settings::current();
    let sta_on = !creds.ssid_str().is_empty();
    let initial = if sta_on {
        let sta = StationConfig::default()
            .with_ssid(creds.ssid_str().try_into().map_err(|_| "sta ssid too long")?)
            .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
                creds.pass_str().try_into().map_err(|_| "sta pass too long")?,
            ));
        Config::AccessPointStation(sta, ap_config)
    } else {
        Config::AccessPoint(ap_config)
    };
    let device = Interface::access_point();
    let mut controller = WifiController::new(
        wifi,
        ControllerConfig::default().with_initial_config(initial),
    )
    .map_err(|_| "wifi controller init failed")?;

    // 弱天线板子的上行信号偏弱，发射功率拉到最大（openc6 同款处理）
    let _ = controller.set_max_tx_power(20);

    let net_config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(gw, 24),
        gateway: Some(gw),
        dns_servers: Default::default(),
    });
    // 5 个常驻 socket：3 serve + dhcp + dns，给到 8 留余量
    static AP_RESOURCES: StaticCell<StackResources<8>> = StaticCell::new();
    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;
    let (stack, runner) = embassy_net::new(
        device,
        net_config,
        AP_RESOURCES.init(StackResources::new()),
        seed,
    );

    spawner.spawn(net_task(runner).expect("spawn net_task"));
    spawner.spawn(manager(controller, sta_on).expect("spawn manager"));
    spawner.spawn(run_dhcp(stack, gw).expect("spawn dhcp"));
    spawner.spawn(dns_hijack(stack, gw).expect("spawn dns"));
    for _ in 0..SERVERS {
        spawner.spawn(serve(stack).expect("spawn serve"));
    }

    // STA netif 无条件建：没凭据也建，否则驱动不支持扫描（openc6 实测结论）
    static STA_RESOURCES: StaticCell<StackResources<2>> = StaticCell::new();
    match Interface::try_station() {
        Some(sta_device) => {
            let (sta, sta_runner) = embassy_net::new(
                sta_device,
                embassy_net::Config::dhcpv4(Default::default()),
                STA_RESOURCES.init(StackResources::new()),
                seed ^ 1,
            );
            spawner.spawn(net_task_sta(sta_runner).expect("spawn net_task_sta"));
            if sta_on {
                spawner.spawn(sta_dhcp(sta).expect("spawn sta_dhcp"));
            }
        }
        None => warn!("sta netif unavailable, scan may fail"),
    }

    WIFI_UP.store(true, Ordering::Relaxed);
    info!(
        "AP `{}` up, password `{}`, portal http://{}/",
        SSID,
        PASSWORD,
        GATEWAY
    );
    Ok(())
}

/// 发起一次扫描；栈没起时信号会挂起，无害
pub fn request_scan() {
    SCAN_REQUEST.signal(());
}

/// 请求分类：portal 页 / 凭据保存 / SD 文件列表 / 其它
#[derive(Debug, PartialEq, Eq)]
enum ReqKind {
    Portal,
    Save(usize),
    Files,
    NotFound,
}

/// 拼一个 `Content-Length` 由正文长度算出的完整响应
fn respond(status: &str, content_type: &str, body: &str) -> String {
    let mut s = String::with_capacity(96 + body.len());
    let _ = write!(
        s,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
        status,
        content_type,
        body.len(),
        body
    );
    s
}

/// 在 80 端口上处理请求，一条连接上连续处理多个请求（keep-alive）
#[embassy_executor::task(pool_size = SERVERS)]
async fn serve(stack: Stack<'static>) -> ! {
    let mut rx_buffer = [0u8; 1536];
    let mut tx_buffer = [0u8; 1536];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(IDLE_TIMEOUT));

        if socket
            .accept(IpListenEndpoint {
                addr: None,
                port: PORT,
            })
            .await
            .is_err()
        {
            socket.abort();
            Timer::after(Duration::from_millis(20)).await;
            continue;
        }

        use embedded_io_async::Write;
        while let Some(req) = read_request(&mut socket).await {
            let response = route(&mut socket, req).await;
            if socket.write_all(response.as_bytes()).await.is_err() {
                break;
            }
            if socket.flush().await.is_err() {
                break;
            }
        }

        socket.close();
        Timer::after(Duration::from_millis(10)).await;
        socket.abort();
    }
}

/// 按请求类别处理：GET 返回 portal 页，POST /save 读体存凭据
async fn route(socket: &mut TcpSocket<'_>, req: ReqKind) -> String {
    match req {
        ReqKind::Save(content_len) => {
            let mut body = [0u8; BODY_MAX];
            let n = content_len.min(BODY_MAX);
            let mut got = 0;
            while got < n {
                match socket.read(&mut body[got..n]).await {
                    Ok(0) | Err(_) => break,
                    Ok(k) => got += k,
                }
            }
            let (ssid, pass) = parse_form(&body[..got]);
            if !ssid.is_empty() {
                settings::update(|c| c.set_wifi_sta(&ssid, &pass));
                info!("sta credentials saved via portal: ssid {}", ssid);
                respond("200 OK", "text/plain", "saved")
            } else {
                respond("400 Bad Request", "text/plain", "missing ssid")
            }
        }
        ReqKind::Portal => respond("200 OK", "text/html", PORTAL_HTML),
        ReqKind::Files => {
            let mut body = String::from("minio-rs sd root\n");
            let listed = crate::sd::with_sd(|sd| {
                sd.list_root(|name, size| {
                    let _ = write!(body, "{} {} bytes\n", name, size);
                })
            });
            match listed {
                Some(Ok(())) => respond("200 OK", "text/plain", &body),
                Some(Err(e)) => respond("500 Internal Server Error", "text/plain", e),
                None => respond("503 Service Unavailable", "text/plain", "sd absent"),
            }
        }
        ReqKind::NotFound => respond("404 Not Found", "text/plain", "not found"),
    }
}

/// 读到请求头结束（`\r\n\r\n`）并分类；外层 `None` 表示连接应结束
async fn read_request(socket: &mut TcpSocket<'_>) -> Option<ReqKind> {
    let mut header = [0u8; 512];
    let mut len = 0;
    while len < header.len() {
        match socket.read(&mut header[len..]).await {
            Ok(0) => return None,
            Ok(n) => {
                len += n;
                if header[..len].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return None,
        }
    }
    if len == 0 {
        return None;
    }

    let text = unsafe { core::str::from_utf8_unchecked(&header[..len]) };
    let first = text.lines().next()?;
    let (method, path) = match first.split_once(' ') {
        Some((m, rest)) => (m, rest.split(' ').next().unwrap_or("/")),
        None => return Some(ReqKind::NotFound),
    };
    let content_len = text
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    match (method, path) {
        ("GET", "/files") => Some(ReqKind::Files),
        ("GET", _) => Some(ReqKind::Portal),
        ("POST", "/save") => Some(ReqKind::Save(content_len)),
        _ => Some(ReqKind::NotFound),
    }
}

/// 解析 application/x-www-form-urlencoded 的 ssid/pass 字段
fn parse_form(body: &[u8]) -> (String, String) {
    let text = core::str::from_utf8(body).unwrap_or("");
    let mut ssid = String::new();
    let mut pass = String::new();
    for pair in text.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "ssid" => ssid = String::from_utf8_lossy(&url_decode(v)).into_owned(),
            "pass" => pass = String::from_utf8_lossy(&url_decode(v)).into_owned(),
            _ => {}
        }
    }
    (ssid, pass)
}

/// `'+'` 转空格、`%XX` 十六进制解码；按字节处理，非 UTF-8 交给调用方取舍
fn url_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h << 4 | l);
                    i += 3;
                }
                _ => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await
}

/// STA 栈的驱动任务
#[embassy_executor::task]
async fn net_task_sta(mut runner: Runner<'static, Interface>) {
    runner.run().await
}

/// 拥有 WifiController 的管理任务：STA 关联/重连 + 扫描请求。
/// 有 portal 存下的凭据时先联 STA（AP 同时保持），没凭据就只服务扫描。
#[embassy_executor::task]
async fn manager(mut controller: WifiController<'static>, sta_on: bool) {
    if sta_on {
        connect_sta(&mut controller).await;
    }
    loop {
        match select(SCAN_REQUEST.wait(), RECONNECT_REQUEST.wait()).await {
            Either::First(()) => scan_once(&mut controller).await,
            Either::Second(()) => {
                warn!("sta link lost, reconnecting");
                connect_sta(&mut controller).await;
            }
        }
    }
}

/// 关联 STA；失败/超时按 5 s 退避重试，直到成功。
/// 注意：本函数会阻塞 manager，重连期间扫描请求无法响应。
/// 没存 STA 凭据时（sta_on = false）根本不会进来，wardriving 不受影响。
async fn connect_sta(controller: &mut WifiController<'static>) {
    loop {
        match with_timeout(STA_CONNECT_TIMEOUT, controller.connect_async()).await {
            Ok(Ok(info)) => {
                info!("sta joined `{}`", info.ssid.as_str());
                return;
            }
            Ok(Err(e)) => warn!("sta connect failed: {}", defmt::Debug2Format(&e)),
            Err(_) => {
                warn!("sta connect timeout");
                // 超时会把 future 丢掉，驱动侧可能仍停在 Connecting；先断开清理再重试
                let _ = with_timeout(STA_RETRY_BACKOFF, controller.disconnect_async()).await;
            }
        }
        Timer::after(STA_RETRY_BACKOFF).await;
    }
}

/// 扫一轮并打印结果
async fn scan_once(controller: &mut WifiController<'static>) {
    let cfg = ScanConfig::default()
        .with_scan_type(ScanTypeConfig::Active {
            min: esp_hal::time::Duration::from_millis(SCAN_DWELL_MIN_MS),
            max: esp_hal::time::Duration::from_millis(SCAN_DWELL_MAX_MS),
        })
        .with_max(SCAN_MAX);
    match controller.scan_async(&cfg).await {
        Ok(list) => {
            info!("wardrive scan done: {} aps", list.len());
            for ap in &list {
                info!(
                    "ap: {} ch{} rssi {}",
                    ap.ssid.as_str(),
                    ap.channel,
                    ap.signal_strength
                );
            }
        }
        Err(e) => warn!("scan failed: {}", defmt::Debug2Format(&e)),
    }
}

/// STA 地址监控：拿到 DHCP 地址就报一次，链路掉线就请求重连。
/// 盯 link 而不是 config：关联成功但拿不到租约时 config 从未 up，
/// 等 config 会永久挂起，重连逻辑形同虚设。
#[embassy_executor::task]
async fn sta_dhcp(stack: Stack<'static>) {
    loop {
        stack.wait_link_up().await;
        match select(stack.wait_config_up(), stack.wait_link_down()).await {
            Either::First(()) => {
                match stack.config_v4() {
                    Some(cfg) => info!("sta dhcp: {}", defmt::Debug2Format(&cfg)),
                    None => warn!("sta dhcp: no config"),
                }
                stack.wait_config_down().await;
                warn!("sta lost address");
            }
            Either::Second(()) => warn!("sta link lost"),
        }
        RECONNECT_REQUEST.signal(());
    }
}

/// 给连上来的设备发 IP
#[embassy_executor::task]
async fn run_dhcp(stack: Stack<'static>, gw: Ipv4Addr) {
    use edge_dhcp::{
        io::{self, DEFAULT_SERVER_PORT},
        server::{Server, ServerOptions},
    };
    use edge_nal::UdpBind;
    use edge_nal_embassy::{Udp, UdpBuffers};

    let mut buf = [0u8; 1500];
    let mut gw_buf = [Ipv4Addr::UNSPECIFIED];

    let buffers = UdpBuffers::<3, 1024, 1024, 10>::new();
    let unbound = Udp::new(stack, &buffers);
    let mut socket = unbound
        .bind(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            DEFAULT_SERVER_PORT,
        )))
        .await
        .unwrap();

    loop {
        let _ = io::server::run(
            &mut Server::<_, 64>::new_with_et(gw),
            &ServerOptions::new(gw, Some(&mut gw_buf)),
            &mut socket,
            &mut buf,
        )
        .await;
        Timer::after(Duration::from_millis(500)).await;
    }
}

/// DNS 劫持：53 端口上把所有 A 查询应答成网关地址，captive 检测因此落到板子
#[embassy_executor::task]
async fn dns_hijack(stack: Stack<'static>, gw: Ipv4Addr) {
    use edge_nal::{UdpBind as _, UdpReceive as _, UdpSend as _};
    use edge_nal_embassy::{Udp, UdpBuffers};

    let mut query = [0u8; 512];
    let mut answer = [0u8; 528];
    let buffers = UdpBuffers::<3, 1024, 1024, 10>::new();
    let unbound = Udp::new(stack, &buffers);
    let Ok(mut socket) = unbound
        .bind(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            53,
        )))
        .await
    else {
        warn!("dns bind failed");
        return;
    };
    let octets = gw.octets();
    info!("dns hijack on :53 -> {}", GATEWAY);
    loop {
        let (n, from) = match socket.receive(&mut query).await {
            Ok(v) => v,
            Err(_) => {
                Timer::after(Duration::from_millis(100)).await;
                continue;
            }
        };
        if let Some(len) = minio_rs_core::dns::answer_a(&query[..n], octets, &mut answer) {
            let _ = socket.send(from, &answer[..len]).await;
        }
    }
}
