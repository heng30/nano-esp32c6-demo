#![no_std]
#![no_main]

extern crate alloc;

use core::fmt::Write as _;
use core::net::Ipv4Addr;
use core::net::SocketAddrV4;
use core::str::FromStr;
use core::sync::atomic::{AtomicBool, Ordering};

use alloc::string::String;
use embassy_executor::Spawner;
use embassy_net::{
    IpListenEndpoint, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4, tcp::TcpSocket,
};
use embassy_time::{Duration, Timer};
use esp_hal::{Blocking, clock::CpuClock, rmt::Rmt, rng::Rng, time::Rate, timer::timg::TimerGroup};
use esp_hal_smartled::{RmtSmartLeds, buffer_size, color_order};
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config, ControllerConfig, Interface, WifiController,
    ap::AccessPointConfig,
};
use smart_leds_trait::{RGB8, SmartLedsWrite};
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

/// 热点 SSID。
const SSID: &str = "esp32c6";
/// 热点密码，WPA2-Personal，至少 8 位。
const PASSWORD: &str = "12345678";
/// 板子自己的地址，也是连上热点后浏览器要访问的地址。
const GATEWAY: &str = "192.168.2.1";
/// HTTP 监听端口。
const PORT: u16 = 80;
/// 并发的连接数：每个实例各占一套收发缓冲区，浏览器会同时开好几条。
const SERVERS: usize = 3;
/// 一条连接的读超时。浏览器会挂 keep-alive 连接不放，超时短一点让出位置给下一条。
const IDLE_TIMEOUT: Duration = Duration::from_secs(3);

/// 有待闪的 `/ping`：HTTP 任务置位、LED 任务取走。
static PING_FLASH: AtomicBool = AtomicBool::new(false);

// 给每条 RTT 日志打上开机微秒时间戳。
defmt::timestamp!("{=u64:us}", embassy_time::Instant::now().as_micros());

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    defmt::error!("panic: {}", defmt::Display2Format(info));
    esp_hal::system::software_reset()
}

#[esp_hal::main]
async fn main(spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 72 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let ap_config = Config::AccessPoint(
        AccessPointConfig::default()
            .with_ssid(SSID.try_into().unwrap())
            .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
                PASSWORD.try_into().unwrap(),
            )),
    );

    let device = Interface::access_point();
    let controller = WifiController::new(
        peripherals.WIFI,
        ControllerConfig::default().with_initial_config(ap_config),
    )
    .unwrap();

    let gw = Ipv4Addr::from_str(GATEWAY).unwrap();
    let net_config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(gw, 24),
        gateway: Some(gw),
        dns_servers: Default::default(),
    });

    static RESOURCES: StaticCell<StackResources<6>> = StaticCell::new();
    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;
    let (stack, runner) = embassy_net::new(
        device,
        net_config,
        RESOURCES.init(StackResources::new()),
        seed,
    );

    let led = {
        let freq = Rate::from_mhz(80);
        let rmt = Rmt::new(peripherals.RMT, freq).unwrap();
        RmtSmartLeds::<{ buffer_size::<RGB8>(1) }, _, RGB8, color_order::Grb>::new(
            esp_hal_smartled::WS2812B_TIMING,
            rmt.channel0,
            peripherals.GPIO8,
            freq,
        )
        .unwrap()
    };

    spawner.spawn(connection(controller).unwrap());
    spawner.spawn(net_task(runner).unwrap());
    spawner.spawn(run_dhcp(stack, gw).unwrap());
    spawner.spawn(led_task(led).unwrap());

    stack.wait_config_up().await;
    defmt::info!(
        "AP `{}` up, password `{}`, try curl http://{}/ping",
        SSID,
        PASSWORD,
        GATEWAY
    );

    for _ in 0..SERVERS {
        spawner.spawn(serve(stack).unwrap());
    }

    core::future::pending::<()>().await;
    unreachable!()
}

/// LED 驱动类型别名，供 `led_task` 的参数使用。
type Led = RmtSmartLeds<'static, { buffer_size::<RGB8>(1) }, Blocking, RGB8, color_order::Grb>;

/// 每收到一次 `/ping` 闪一下：亮 80 ms 后熄灭，没有请求时保持全黑。
#[embassy_executor::task]
async fn led_task(mut led: Led) {
    loop {
        if PING_FLASH.swap(false, Ordering::Relaxed) {
            let _ = led.write([RGB8 { r: 0, g: 16, b: 64 }].into_iter());
            Timer::after(Duration::from_millis(80)).await;
            let _ = led.write([RGB8 { r: 0, g: 0, b: 0 }].into_iter());
        }
        Timer::after(Duration::from_millis(20)).await;
    }
}

/// 在 80 端口上处理请求，一条连接上连续处理多个请求（keep-alive）。
///
/// 同时跑 `SERVERS` 个实例，否则一个挂着的 keep-alive 连接会把其它连接全卡在 accept 队列里。
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
        while let Some(is_ping) = read_request(&mut socket).await {
            let response = if is_ping {
                PING_FLASH.store(true, Ordering::Relaxed);
                respond("200 OK", "text/plain", "pong")
            } else {
                respond("404 Not Found", "text/plain", "not found")
            };

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

/// 拼一个 `Content-Length` 由正文长度算出的完整响应。
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

/// 读到请求头结束（`\r\n\r\n`），返回请求路径是否为 `/ping`。
///
/// 外层 `None` 表示对端关了连接或读失败，调用方应结束这条 keep-alive 连接。
async fn read_request(socket: &mut TcpSocket<'_>) -> Option<bool> {
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
    Some(matches!(text.lines().next(), Some(line) if line.starts_with("GET /ping")))
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await
}

/// 起 AP 之后等客户端；这里只负责让 controller 活着，不跟踪客户端信息。
#[embassy_executor::task]
async fn connection(controller: WifiController<'static>) {
    loop {
        match controller
            .wait_for_access_point_connected_event_async()
            .await
        {
            Ok(esp_radio::wifi::ap::EventInfo::Connected(info)) => {
                defmt::info!("station joined: {:02x}", info.mac)
            }
            Ok(esp_radio::wifi::ap::EventInfo::Disconnected(info)) => {
                defmt::info!("station left: {:02x}", info.mac)
            }
            Err(e) => defmt::info!("event error: {}", defmt::Debug2Format(&e)),
        }
    }
}

/// 给连上来的设备发 IP，省得手动配静态地址。
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
        .bind(core::net::SocketAddr::V4(SocketAddrV4::new(
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
