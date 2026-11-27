//! 把 kk-ui-rs 的模拟例子（`examples/sim`）移植到 SSD1306 128×64 OLED 上。
//!
//! 画布就是屏的 GDDRAM：128×64 的 1 bit 帧缓冲整块递交，不放大也不转换位序。
//! 输入是外接的三个按键（上 / 下 / 确认），内部上拉、按下为低。

#![no_std]
#![no_main]

mod app;
mod display;

use defmt::info;
use embassy_executor::Spawner;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    spi::master::{Config as SpiConfig, Spi},
    time::{Instant, Rate},
};
use kk_ui::{Frame, Input as UiInput, KEY_DOWN, KEY_OK, KEY_UP, KkUi};
use panic_rtt_target as _;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

/// 逻辑画布宽度（像素），与屏宽一致。
const CANVAS_WIDTH: i16 = display::WIDTH as i16;
/// 逻辑画布高度（像素），与屏高一致。
const CANVAS_HEIGHT: i16 = display::HEIGHT as i16;
/// SPI 时钟：SSD1306 上限 10 MHz。
const SPI_FREQ_MHZ: u32 = 8;
/// 按键消抖时间。
const DEBOUNCE_MS: u32 = 15;
/// 主循环节拍。
const TICK_MS: u32 = 5;
/// 状态统计日志的间隔。
const LOG_INTERVAL_MS: u32 = 5000;

/// 帧缓冲：`width * ceil(height / 8)` 字节，由 UI 独占。
static FRAME_BUFFER: StaticCell<[u8; Frame::buffer_len(CANVAS_WIDTH, CANVAS_HEIGHT)]> =
    StaticCell::new();

/// 三个按键：内部上拉、按下为低，原始电平连续稳定 [`DEBOUNCE_MS`] 才生效。
struct Buttons {
    /// 上、下、确认三根引脚，下标即位序。
    pins: [Input<'static>; 3],
    /// 上一次采样到的原始位掩码。
    raw: u8,
    /// 原始位掩码变化的时刻（毫秒）。
    raw_at: u32,
    /// 消抖后的位掩码。
    stable: u8,
}

impl Buttons {
    fn new(up: Input<'static>, down: Input<'static>, ok: Input<'static>) -> Self {
        Self {
            pins: [up, down, ok],
            raw: 0,
            raw_at: 0,
            stable: 0,
        }
    }

    /// 采样一次，返回这一拍上报给 UI 的按键位掩码。
    fn keys(&mut self, now: u32) -> u8 {
        let mut raw = 0u8;
        for (index, pin) in self.pins.iter().enumerate() {
            if pin.is_low() {
                raw |= 1 << index;
            }
        }
        if raw != self.raw {
            self.raw = raw;
            self.raw_at = now;
        }
        if self.raw != self.stable && now.wrapping_sub(self.raw_at) >= DEBOUNCE_MS {
            self.stable = self.raw;
        }

        let mut keys = 0u8;
        if self.stable & 0b001 != 0 {
            keys |= KEY_UP;
        }
        if self.stable & 0b010 != 0 {
            keys |= KEY_DOWN;
        }
        if self.stable & 0b100 != 0 {
            keys |= KEY_OK;
        }
        keys
    }
}

/// 当前页面在 `APP.routes` 里的下标 + 1，`0` 表示尚未初始化；路由句柄本身给不了 defmt。
fn page_slot(page: Option<&'static kk_ui::PageRoute>) -> u8 {
    let Some(page) = page else { return 0 };
    match app::APP
        .routes
        .iter()
        .position(|route| core::ptr::eq(*route, page))
    {
        Some(index) => index as u8 + 1,
        None => 0,
    }
}

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let mut delay = Delay::new();

    // 接线见 README：SCLK=GPIO6、MOSI=GPIO7、CS=GPIO0、DC=GPIO1、RES=GPIO2。
    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default().with_frequency(Rate::from_mhz(SPI_FREQ_MHZ)),
    )
    .expect("spi config")
    .with_sck(peripherals.GPIO6)
    .with_mosi(peripherals.GPIO7);

    let mut display = display::Display::new(
        spi,
        Output::new(peripherals.GPIO1, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO2, Level::High, OutputConfig::default()),
        Output::new(peripherals.GPIO0, Level::High, OutputConfig::default()),
        &mut delay,
    );

    // 三个按键：UP=GPIO10、DOWN=GPIO11、OK=GPIO18，另一头接 GND。
    let input = InputConfig::default().with_pull(Pull::Up);
    let mut buttons = Buttons::new(
        Input::new(peripherals.GPIO10, input),
        Input::new(peripherals.GPIO11, input),
        Input::new(peripherals.GPIO18, input),
    );

    let buffer = FRAME_BUFFER.init([0u8; Frame::buffer_len(CANVAS_WIDTH, CANVAS_HEIGHT)]);
    let mut ui = KkUi::new(&app::APP, buffer, CANVAS_WIDTH, CANVAS_HEIGHT);
    let now = 0u32;
    let status = ui.init();
    info!(
        "kk_ui init: {} canvas={}x{}",
        status.as_str(),
        CANVAS_WIDTH,
        CANVAS_HEIGHT
    );
    app::reset(&mut ui, now);

    let mut last_log = 0u32;
    let mut frames = 0u32;
    let mut flush_us = 0u64;
    let mut flushes = 0u32;

    loop {
        let now = Instant::now().duration_since_epoch().as_millis() as u32;

        ui.update(
            now,
            UiInput {
                keys: buttons.keys(now),
                encoder_delta: 0,
            },
        );
        app::update(&mut ui, now);
        app::process_events(&mut ui);

        if ui.take_frame_ready() {
            let started = Instant::now();
            display.flush(ui.frame().buffer());
            flush_us += started.elapsed().as_micros();
            flushes += 1;
            if frames == 0 {
                info!("first frame flushed");
            }
            frames += 1;
        }
        if now.wrapping_sub(last_log) >= LOG_INTERVAL_MS {
            last_log = now;
            let average = if flushes == 0 {
                0
            } else {
                flush_us / flushes as u64
            };
            info!(
                "t={}ms page={} sel={} dialog={} frames={} flush={}us",
                now,
                page_slot(ui.current_page()),
                ui.selected_index(),
                ui.dialog_open(),
                frames,
                average
            );
            flush_us = 0;
            flushes = 0;
        }
        delay.delay_millis(TICK_MS);
    }
}
