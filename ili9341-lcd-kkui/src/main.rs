//! 在 ILI9341 上跑 KK_UI 标准 Demo：160×120 的 1 bit 画布按 2 倍放大铺满整屏。
//!
//! 板上只有一个 BOOT 键（GPIO9）：短按当「确定」，按住超过 [`LONG_PRESS_MS`] 当
//! 「向下」；另外定时注入一串自动演示按键，没人操作时界面也会自己走一遍。

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
    time::Instant,
};
use kk_ui::{Frame, Input as UiInput, KEY_DOWN, KEY_OK, KkUi};
use panic_rtt_target as _;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

/// 逻辑画布宽度（像素），`× SCALE` 就是屏宽。
const CANVAS_WIDTH: i16 = 160;
/// 逻辑画布高度（像素），`× SCALE` 就是屏高。
const CANVAS_HEIGHT: i16 = 120;
/// BOOT 按住超过该时长按「向下」处理。
const LONG_PRESS_MS: u32 = 700;
/// 按键消抖时间。
const DEBOUNCE_MS: u32 = 15;
/// 一次注入按键的保持时长，必须大于 KK_UI 的 20 ms 消抖。
const KEY_HOLD_MS: u32 = 60;
/// 两次自动演示按键的间隔。编辑器类弹窗要「进编辑 → 提交」两下确定，
/// 间隔过大时整个设置页要几十秒才走得完，看起来像卡住。
const AUTOPLAY_INTERVAL_MS: u32 = 500;
/// 主循环节拍。
const TICK_MS: u32 = 5;
/// 自动演示状态（页面 / 选中行 / 弹窗）连续多久不变就判定为卡住。
/// 正常最长只会静止一两秒（编辑器进编辑→提交、弹窗开关动画）。
const STUCK_MS: u32 = 15_000;

/// 帧缓冲：`width * ceil(height / 8)` 字节，由 UI 独占。
static FRAME_BUFFER: StaticCell<[u8; Frame::buffer_len(CANVAS_WIDTH, CANVAS_HEIGHT)]> =
    StaticCell::new();

/// 把一次按键展开成持续 [`KEY_HOLD_MS`] 的电平。
struct KeyInjector {
    keys: u8,
    until: u32,
}

impl KeyInjector {
    const fn new() -> Self {
        Self { keys: 0, until: 0 }
    }

    fn pulse(&mut self, now: u32, keys: u8) {
        self.keys = keys;
        self.until = now.wrapping_add(KEY_HOLD_MS);
    }

    fn keys(&self, now: u32) -> u8 {
        if self.keys != 0 && (now.wrapping_sub(self.until) as i32) < 0 {
            self.keys
        } else {
            0
        }
    }
}

/// BOOT 键的消抖与长短按判定。
struct Button {
    raw: bool,
    raw_at: u32,
    stable: bool,
    pressed_at: Option<u32>,
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
                injector.pulse(now, KEY_DOWN);
            }
            (Some(_), false) => {
                self.pressed_at = None;
                if !self.long_sent {
                    injector.pulse(now, KEY_OK);
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

    let boot = Input::new(
        peripherals.GPIO9,
        InputConfig::default().with_pull(Pull::Up),
    );

    let buffer = FRAME_BUFFER.init([0u8; Frame::buffer_len(CANVAS_WIDTH, CANVAS_HEIGHT)]);
    let mut ui = KkUi::new(&app::APP, buffer, CANVAS_WIDTH, CANVAS_HEIGHT);
    let now = 0u32;
    let status = ui.init();
    info!(
        "kk_ui init: {} canvas={}x{} scale={}",
        status.as_str(),
        CANVAS_WIDTH,
        CANVAS_HEIGHT,
        display::SCALE
    );
    app::reset(&mut ui, now);

    let mut button = Button::new();
    let mut injector = KeyInjector::new();
    let mut autoplay = app::Autoplay::new();
    let mut next_autoplay = AUTOPLAY_INTERVAL_MS;
    let mut last_log = 0u32;
    let mut frames = 0u32;
    let mut blit_us = 0u64;
    let mut flushes = 0u32;
    let mut last_state = (0u8, 0u16, false);
    let mut last_change = 0u32;

    loop {
        let now = Instant::now().duration_since_epoch().as_millis() as u32;

        if now.wrapping_sub(next_autoplay) as i32 >= 0 {
            next_autoplay = now.wrapping_add(AUTOPLAY_INTERVAL_MS);
            let key = autoplay.next_key(&ui);
            info!(
                "autoplay page={} sel={} dialog={} key={}",
                page_slot(ui.current_page()),
                ui.selected_index(),
                ui.dialog_open(),
                key
            );
            injector.pulse(now, key);
        }
        let down = button.update(now, boot.is_low());
        button.poll(now, down, &mut injector);
        let input = UiInput {
            keys: injector.keys(now),
            encoder_delta: 0,
        };

        ui.update(now, input);
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
            display::blit_canvas(&mut display, ui.frame());
            blit_us += started.elapsed().as_micros();
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
                blit_us / flushes as u64
            };
            info!(
                "t={}ms page={} sel={} dialog={} still={} frames={} blit={}us",
                now,
                page_slot(ui.current_page()),
                ui.selected_index(),
                ui.dialog_open(),
                now.wrapping_sub(last_change),
                frames,
                average
            );
            blit_us = 0;
            flushes = 0;
        }
        delay.delay_millis(TICK_MS);
    }
}
