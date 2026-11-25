#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use defmt::info;
use embassy_executor::Spawner;
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    timer::timg::TimerGroup,
    uart::{Config as UartConfig, RxConfig, Uart},
    usb::usb_serial_jtag::UsbSerialJtag,
};
use panic_rtt_target as _;

mod display;
mod link;

esp_bootloader_esp_idf::esp_app_desc!();

/// 渲染任务：从队列取块画屏。
/// UART0 链路的波特率，上位机连 CH343 时必须用同一个值（`--baud`）。
const UART_BAUD: u32 = 2_000_000;
/// RX FIFO 认满阈值（字节）。esp-hal 默认 120，硬件 FIFO 只有 128，只剩 8 字节
/// （2 Mbaud 下 40µs）余量；渲染画一块 tile 约 200µs 不让出，收发重叠时必溢出丢整批。
/// 16 给 (128-16)×5µs = 560µs 余量。
const RX_FIFO_FULL_THRESHOLD: u16 = 16;


/// USB CDC 链路的收帧任务。
/// 渲染任务：从队列取块画屏（单一所有者）。
#[embassy_executor::task]
async fn render(display: display::Display) {
    link::render(display).await
}

#[embassy_executor::task]
async fn serve_usb(mut port: UsbSerialJtag<'static, esp_hal::Async>) {
    link::serve(&mut port).await
}

/// UART0（板载 CH343P）链路的收帧任务。
#[embassy_executor::task]
async fn serve_uart(mut port: Uart<'static, esp_hal::Async>) {
    link::serve(&mut port).await
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

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

    // 收帧（两条链路各一个任务）与渲染解耦：收到块就入队、立刻回 ACK，
    // 渲染任务单一所有者画屏，链路之间不再抢锁。
    let usb = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async();
    let uart = Uart::new(
        peripherals.UART0,
        UartConfig::default()
            .with_baudrate(UART_BAUD)
            .with_rx(
                RxConfig::default().with_fifo_full_threshold(RX_FIFO_FULL_THRESHOLD),
            ),
    )
    .expect("uart init failed")
    .with_rx(peripherals.GPIO17)
    .with_tx(peripherals.GPIO16)
    .into_async();
    info!(
        "secondary screen ready, links: usb cdc + uart0 {} baud",
        UART_BAUD
    );
    link::init_pool();
    spawner.spawn(render(display).expect("render task"));
    spawner.spawn(serve_usb(usb).expect("usb task"));
    spawner.spawn(serve_uart(uart).expect("uart task"));

    core::future::pending().await

}
