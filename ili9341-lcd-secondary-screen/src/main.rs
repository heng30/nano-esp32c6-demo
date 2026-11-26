#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

extern crate alloc;

use defmt::info;
use embassy_executor::Spawner;
use embassy_net::{Runner, Stack, StackResources, tcp::TcpSocket};
use embassy_time::{Duration, Timer};
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    rng::Rng,
    timer::timg::TimerGroup,
    uart::{Config as UartConfig, RxConfig, Uart},
    usb::usb_serial_jtag::UsbSerialJtag,
};
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config, ControllerConfig, Interface, WifiController,
    sta::StationConfig,
};
use panic_rtt_target as _;
use static_cell::StaticCell;

mod display;
mod link;

esp_bootloader_esp_idf::esp_app_desc!();

/// UART0 链路的波特率，上位机连 CH343 时必须用同一个值（`--baud`）。
const UART_BAUD: u32 = 2_000_000;
/// RX FIFO 认满阈值（字节）。esp-hal 默认 120，硬件 FIFO 只有 128，只剩 8 字节
/// （2 Mbaud 下 40µs）余量；渲染画一块 tile 约 200µs 不让出，收发重叠时必溢出丢整批。
/// 16 给 (128-16)×5µs = 560µs 余量。
const RX_FIFO_FULL_THRESHOLD: u16 = 16;
/// 要连的热点。
const SSID: &str = "R3";
/// 热点口令（WPA2-Personal）。
const PASSWORD: &str = "visec*admin";
/// WiFi 链路的监听端口，上位机 `wifi --port` 必须用同一个值。
const TCP_PORT: u16 = 8080;
/// TCP 接收缓冲（≈ smoltcp 的接收窗口）：一条消息最大约 16.6 KB，reader 分几次读拼一条。
const TCP_RX_BUF: usize = 16384;
/// TCP 发送缓冲：应答最长 18 字节。
const TCP_TX_BUF: usize = 512;
/// 一条连接允许的最长静默，超时后 socket 自动断开，上位机重连即可。
const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);
/// WiFi 协议栈的堆：radio 与 embassy-net 的分配都从这里来。
const HEAP_SIZE: usize = 72 * 1024;

/// 渲染任务：从队列取块画屏（单一所有者）。
#[embassy_executor::task]
async fn render(display: display::Display) {
    link::render(display).await
}

/// USB CDC 链路的收帧任务。
#[embassy_executor::task]
async fn serve_usb(mut port: UsbSerialJtag<'static, esp_hal::Async>) {
    loop {
        link::serve(&mut port).await;
    }
}

/// UART0（板载 CH343P）链路的收帧任务。
#[embassy_executor::task]
async fn serve_uart(mut port: Uart<'static, esp_hal::Async>) {
    loop {
        link::serve(&mut port).await;
    }
}

/// WiFi 链路：板子做 TCP 服务端，上位机连上来后跑同一套帧协议。
/// 一条连接结束（对端断开或出错）就重新监听，不重启板子。
#[embassy_executor::task]
async fn serve_wifi(stack: Stack<'static>) -> ! {
    stack.wait_config_up().await;
    match stack.config_v4() {
        Some(config) => {
            let octets = config.address.address().octets();
            info!(
                "wifi up, tcp server on {}.{}.{}.{}:{}",
                octets[0], octets[1], octets[2], octets[3], TCP_PORT
            );
        }
        None => info!("wifi up without address"),
    }

    loop {
        // 缓冲与 socket 同寿命，放在循环里让它每轮重新借出
        let mut rx = [0u8; TCP_RX_BUF];
        let mut tx = [0u8; TCP_TX_BUF];
        let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
        socket.set_timeout(Some(SOCKET_TIMEOUT));
        if socket
            .accept(embassy_net::IpListenEndpoint {
                addr: None,
                port: TCP_PORT,
            })
            .await
            .is_err()
        {
            socket.abort();
            Timer::after_millis(50).await;
            continue;
        }
        info!("wifi link connected");
        link::serve(&mut socket).await;
        socket.close();
        Timer::after_millis(50).await;
    }
}#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await
}
/// 维持 WiFi 连接：掉线、或一开始就关联失败，等 2 秒重试。
#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    loop {
        match controller.connect_async().await {
            Ok(info) => info!("joined `{}`", info.ssid),
            Err(error) => defmt::warn!("connect failed: {}", defmt::Debug2Format(&error)),
        }
        let _ = controller.wait_for_disconnect_async().await;
        defmt::warn!("wifi down, retrying");
        Timer::after_millis(2000).await;
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: HEAP_SIZE);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let mut delay = Delay::new();

    // 顺序必须与 display.rs 里的 DATA_PINS 一致
    let data = [
        Output::new(peripherals.GPIO0, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO1, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO2, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO3, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO4, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO5, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO6, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO7, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO8, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO10, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO11, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO18, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO19, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO20, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO21, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO22, Level::Low, OutputConfig::default()),
    ];

    let mut display = display::new(
        data,
        Output::new(peripherals.GPIO15, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO23, Level::High, OutputConfig::default()),
        &mut delay,
    );
    display.clear(Rgb565::BLACK).expect("clear failed");



    // 收帧（三条链路各一个任务）与渲染解耦：收到块就入队，ACK 等渲染腾出下一块槽
    // 再回——背压精确到渲染吞吐；渲染任务单一所有者画屏，链路之间不抢锁。
    let usb = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async();
    let uart = Uart::new(
        peripherals.UART0,
        UartConfig::default()
            .with_baudrate(UART_BAUD)
            .with_rx(RxConfig::default().with_fifo_full_threshold(RX_FIFO_FULL_THRESHOLD)),
    )
    .expect("uart init failed")
    .with_rx(peripherals.GPIO17)
    .with_tx(peripherals.GPIO16)
    .into_async();

    let station = StationConfig::default()
        .with_ssid(SSID.try_into().expect("ssid too long"))
        .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
            PASSWORD.try_into().expect("password too long"),
        ));
    let controller = WifiController::new(
        peripherals.WIFI,
        ControllerConfig::default().with_initial_config(Config::Station(station)),
    )
    .expect("wifi init failed");

    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;
    let (stack, runner) = embassy_net::new(
        Interface::station(),
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );

    info!(
        "secondary screen ready, links: usb cdc + uart0 {} baud + wifi `{}` tcp {}",
        UART_BAUD, SSID, TCP_PORT
    );
    link::init_pool();
    spawner.spawn(render(display).expect("render task"));
    spawner.spawn(serve_usb(usb).expect("usb task"));
    spawner.spawn(serve_uart(uart).expect("uart task"));
    spawner.spawn(connection(controller).expect("wifi task"));
    spawner.spawn(net_task(runner).expect("net task"));
    spawner.spawn(serve_wifi(stack).expect("wifi link task"));

    core::future::pending().await
}
