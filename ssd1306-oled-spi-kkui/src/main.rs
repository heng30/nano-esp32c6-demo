//! 把 kk-ui-rs 的模拟例子（`examples/sim`）移植到 SSD1306 128×64 OLED 上。
//!
//! 画布就是屏的 GDDRAM：128×64 的 1 bit 帧缓冲整块递交，不放大也不转换位序。
//! 板上只有一个 BOOT 键（GPIO9）：短按当「确定」，按住超过 [`LONG_PRESS_MS`] 当
//! 「向下」；没人操作时 `app::Autoplay` 每 [`AUTOPLAY_INTERVAL_MS`] 注入一次按键，
//! 界面会自己走遍各页与各类弹窗。

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
use kk_ui::{Frame, Input as UiInput, KEY_DOWN, KEY_OK, KkUi};
use panic_rtt_target as _;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

/// 逻辑画布宽度（像素），与屏宽一致。
const CANVAS_WIDTH: i16 = display::WIDTH as i16;
/// 逻辑画布高度（像素），与屏高一致。
const CANVAS_HEIGHT: i16 = display::HEIGHT as i16;
/// SPI 时钟：SSD1306 上限 10 MHz。
const SPI_FREQ_MHZ: u32 = 8;
/// BOOT 按住超过该时长按「向下」处理。
const LONG_PRESS_MS: u32 = 700;
/// 按键消抖时间。
const DEBOUNCE_MS: u32 = 15;
/// 一次注入按键的保持时长，必须大于 KK_UI 的 20 ms 消抖。
const KEY_HOLD_MS: u32 = 60;
/// 两次自动演示按键的间隔。
const AUTOPLAY_INTERVAL_MS: u32 = 500;
/// 主循环节拍。
const TICK_MS: u32 = 5;
/// 自动演示状态（页面 / 选中行 / 弹窗）连续多久不变就判定为卡住。
const STUCK_MS: u32 = 15_000;

/// 帧缓冲：`width * ceil(height / 8)` 字节，由 UI 独占。
static FRAME_BUFFER: StaticCell<[u8; Frame::buffer_len(CANVAS_WIDTH, CANVAS_HEIGHT)]> =
    StaticCell::new();

/// 把一次按键与编码器增量展开成持续 [`KEY_HOLD_MS`] 的电平。
struct KeyInjector {
    /// 按住期间上报的按键位掩码。
    keys: u8,
    /// 待上报的编码器增量，只发一拍。
    encoder: i16,
    /// 松开时刻（毫秒）。
    until: u32,
}

impl KeyInjector {
    const fn new() -> Self {
        Self {
            keys: 0,
            encoder: 0,
            until: 0,
        }
    }

    fn pulse(&mut self, now: u32, keys: u8, encoder: i16) {
        self.keys = keys;
        self.encoder = encoder;
        self.until = now.wrapping_add(KEY_HOLD_MS);
    }

    /// 取出这一拍该上报的输入；编码器增量是事件，取出即清零。
    fn input(&mut self, now: u32) -> UiInput {
        let held = (now.wrapping_sub(self.until) as i32) < 0;
        let encoder = core::mem::take(&mut self.encoder);
        UiInput {
            keys: if held { self.keys } else { 0 },
            encoder_delta: if held { encoder } else { 0 },
        }
    }
}

/// BOOT 键的消抖与长短按判定。
struct Button {
    /// 上一次采样到的原始电平（按下为 true）。
    raw: bool,
    /// 原始电平变化的时刻（毫秒）。
    raw_at: u32,
    /// 消抖后的电平。
    stable: bool,
    /// 按下时刻（毫秒），松开后清空。
    pressed_at: Option<u32>,
    /// 本次按住是否已经发过「向下」。
    long_sent: bool,
}

impl Button {
    const fn new() -> Self {
        Self {
            raw: false,
            raw_at: 0,
            stable: false,
            pressed_at: None,
            long_sent: false,
        }
    }

    fn update(&mut self, now: u32, raw: bool) -> bool {
        if raw != self.raw {
            self.raw = raw;
            self.raw_at = now;
        }
        if self.raw != self.stable && now.wrapping_sub(self.raw_at) >= DEBOUNCE_MS {
            self.stable = self.raw;
        }
        self.stable
    }

    fn poll(&mut self, now: u32, down: bool, injector: &mut KeyInjector) {
        match (self.pressed_at, down) {
            (None, true) => {
                self.pressed_at = Some(now);
                self.long_sent = false;
            }
            (Some(start), true) if !self.long_sent && now.wrapping_sub(start) >= LONG_PRESS_MS => {
                self.long_sent = true;
                injector.pulse(now, KEY_DOWN, 0);
            }
            (Some(_), false) => {
                self.pressed_at = None;
                if !self.long_sent {
                    injector.pulse(now, KEY_OK, 0);
                }
            }
            _ => {}
        }
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

    let boot = Input::new(
        peripherals.GPIO9,
        InputConfig::default().with_pull(Pull::Up),
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

    let mut button = Button::new();
    let mut injector = KeyInjector::new();
    let mut autoplay = app::Autoplay::new();
    let mut next_autoplay = AUTOPLAY_INTERVAL_MS;
    let mut last_log = 0u32;
    let mut frames = 0u32;
    let mut flush_us = 0u64;
    let mut flushes = 0u32;
    let mut last_state = (0u8, 0u16, false);
    let mut last_change = 0u32;

    loop {
        let now = Instant::now().duration_since_epoch().as_millis() as u32;

        if now.wrapping_sub(next_autoplay) as i32 >= 0 {
            next_autoplay = now.wrapping_add(AUTOPLAY_INTERVAL_MS);
            let (key, encoder) = autoplay.next_key(&ui);
            info!(
                "autoplay page={} sel={} dialog={} key={} enc={}",
                page_slot(ui.current_page()),
                ui.selected_index(),
                ui.dialog_open(),
                key,
                encoder
            );
            injector.pulse(now, key, encoder);
        }
        let down = button.update(now, boot.is_low());
        button.poll(now, down, &mut injector);

        ui.update(now, injector.input(now));
        app::update(&mut ui, now);
        app::process_events(&mut ui);

        let state = (
            page_slot(ui.current_page()),
            ui.selected_index(),
            ui.dialog_open(),
        );
        if state != last_state {
            last_state = state;
            last_change = now;
        } else if now.wrapping_sub(last_change) >= STUCK_MS {
            last_change = now;
            info!(
                "autoplay stuck at page={} sel={} dialog={}, resetting",
                state.0, state.1, state.2
            );
            autoplay = app::Autoplay::new();
        }
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
        if now.wrapping_sub(last_log) >= 5000 {
            last_log = now;
            let average = if flushes == 0 {
                0
            } else {
                flush_us / flushes as u64
            };
            info!(
                "t={}ms page={} sel={} dialog={} still={} frames={} flush={}us",
                now,
                page_slot(ui.current_page()),
                ui.selected_index(),
                ui.dialog_open(),
                now.wrapping_sub(last_change),
                frames,
                average
            );
            flush_us = 0;
            flushes = 0;
        }
        delay.delay_millis(TICK_MS);
    }
}
