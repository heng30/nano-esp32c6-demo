#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_net::{Runner, Stack, StackResources, dns::DnsQueryType, tcp::TcpSocket};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use embedded_tls::{Aes128GcmSha256, TlsConfig, TlsConnection, TlsContext, UnsecureProvider};
use esp_hal::{
    clock::CpuClock,
    rng::{Trng, TrngSource},
    timer::timg::TimerGroup,
};
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config, ControllerConfig, Interface, WifiController,
    sta::StationConfig,
};
use static_cell::StaticCell;

use panic_rtt_target as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// 要连的路由器。
const SSID: &str = "R3";
/// WPA2 口令。
const PASSWORD: &str = "visec*admin";
/// 接口域名，同时作为 TLS 的 SNI。
const HOST: &str = "api.bilibili.com";
/// HTTPS 端口。
const HTTPS_PORT: u16 = 443;
/// 直接手写的请求，`vmid=52374219` 是要查的用户 UID。
const REQUEST: &str = concat!(
    "GET /x/relation/stat?vmid=52374219 HTTP/1.1\r\n",
    "Host: api.bilibili.com\r\n",
    "User-Agent: esp32c6-follower/0.1\r\n",
    "Connection: close\r\n\r\n"
);
/// 查询间隔。接口本身按分钟刷新，更密没有意义。
const INTERVAL: Duration = Duration::from_secs(60);
/// TCP 读写超时，DNS 抽风时别把任务挂死。
const SOCKET_TIMEOUT: Duration = Duration::from_secs(20);
/// TLS 记录读缓冲，一条 TLS 记录最大 16640 字节。
const TLS_READ_BUF: usize = 16640;
/// TLS 记录写缓冲，够放一个握手记录即可。
const TLS_WRITE_BUF: usize = 4096;
/// TCP 收发缓冲。
const TCP_BUF: usize = 1024;
/// 收包缓冲，这个接口整包不到 200 字节。
const RESPONSE_BUF: usize = 1024;
/// 接口的一次真实返回，用于解析自检。
const SAMPLE_BODY: &[u8] = br#"{"code":0,"message":"OK","ttl":1,"data":{"mid":52374219,"following":7,"whisper":0,"black":1,"follower":5054,"fans_medal_toast":null,"fans_effect":null}}"#;

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();
    debug_assert_eq!(parse_follower(SAMPLE_BODY), Some(5054));

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // TLS 握手里的 RSA 验签要在大整数上分配，给宽一点。
    esp_alloc::heap_allocator!(size: 96 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    // 真随机源要一直活着，TLS 的密钥材料靠它。
    let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);
    let trng = Trng::try_new().unwrap();

    let sta_config = StationConfig::default()
        .with_ssid(SSID.try_into().unwrap())
        .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
            PASSWORD.try_into().unwrap(),
        ));
    let controller = WifiController::new(
        peripherals.WIFI,
        ControllerConfig::default().with_initial_config(Config::Station(sta_config)),
    )
    .unwrap();

    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(
        Interface::station(),
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        trng.random() as u64 | (trng.random() as u64) << 32,
    );

    spawner.spawn(connection(controller).unwrap());
    spawner.spawn(net_task(runner).unwrap());
    spawner.spawn(follower(stack, trng).unwrap());

    stack.wait_config_up().await;
    defmt::info!(
        "dhcp up: {}",
        defmt::Debug2Format(&stack.config_v4().unwrap())
    );

    core::future::pending::<()>().await;
    unreachable!()
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await
}

/// 连上之后只盯掉线；掉线、或一开始就关联失败，就等 2 秒重试。
#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    loop {
        match controller.connect_async().await {
            Ok(info) => defmt::info!("joined `{}`", info.ssid),
            Err(e) => defmt::error!("connect failed: {}", defmt::Debug2Format(&e)),
        }
        let _ = controller.wait_for_disconnect_async().await;
        defmt::warn!("wifi down, retrying");
        Timer::after(Duration::from_secs(2)).await;
    }
}

/// 每分钟查一次关注数。
#[embassy_executor::task]
async fn follower(stack: Stack<'static>, trng: Trng) {
    stack.wait_config_up().await;
    loop {
        fetch_follower(stack, trng.clone()).await;
        Timer::after(INTERVAL).await;
    }
}

/// 走 HTTPS 取一次关注数，成功就打一行日志。
async fn fetch_follower(stack: Stack<'static>, trng: Trng) {
    let mut rx_buf = [0u8; TCP_BUF];
    let mut tx_buf = [0u8; TCP_BUF];
    let mut tls_read = [0u8; TLS_READ_BUF];
    let mut tls_write = [0u8; TLS_WRITE_BUF];
    let mut response = [0u8; RESPONSE_BUF];

    let addr = match stack
        .dns_query(HOST, DnsQueryType::A)
        .await
        .map(|a| a.first().copied())
    {
        Ok(Some(addr)) => addr,
        Ok(None) => return defmt::error!("no A record for {}", HOST),
        Err(e) => return defmt::error!("dns failed: {}", defmt::Debug2Format(&e)),
    };

    let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
    socket.set_timeout(Some(SOCKET_TIMEOUT));
    if let Err(e) = socket.connect((addr, HTTPS_PORT)).await {
        socket.abort();
        return defmt::error!("tcp connect failed: {}", defmt::Debug2Format(&e));
    }

    // shortcut: 不校验证书链（no_std 下没有可信时间和根证书），要防中间人得换成带 verifier 的 provider。
    let config = TlsConfig::new().with_server_name(HOST);
    let mut tls = TlsConnection::new(socket, &mut tls_read, &mut tls_write);
    if let Err(e) = tls
        .open(TlsContext::new(
            &config,
            UnsecureProvider::new::<Aes128GcmSha256>(trng),
        ))
        .await
    {
        return defmt::error!("tls handshake failed: {}", defmt::Debug2Format(&e));
    }

    if tls.write_all(REQUEST.as_bytes()).await.is_err() || tls.flush().await.is_err() {
        return defmt::error!("send request failed");
    }

    let mut len = 0;
    while len < response.len() {
        match tls.read(&mut response[len..]).await {
            Ok(0) => break,
            Ok(n) => len += n,
            Err(_) => break,
        }
    }

    match parse_follower(&response[..len]) {
        Some(follower) => defmt::info!("follower: {}", follower),
        None => defmt::error!("no follower field in response: {:?}", &response[..len]),
    }
}

/// 从 JSON 里抠出 `"follower":<数字>`，返回关注者数。
fn parse_follower(json: &[u8]) -> Option<u32> {
    const KEY: &[u8] = b"\"follower\":";
    let start = json.windows(KEY.len()).position(|w| w == KEY)? + KEY.len();
    json[start..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(None, |acc, b| {
            Some(acc.unwrap_or(0) * 10 + u32::from(b - b'0'))
        })
}
