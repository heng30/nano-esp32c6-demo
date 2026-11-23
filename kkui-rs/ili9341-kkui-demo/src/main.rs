//! 在 ILI9341 上跑 KK_UI 标准 Demo：1 bit 画布按 2 倍放大居中显示。
//!
//! 板上只有一个 BOOT 键（GPIO9）：短按当「确定」，按住超过 [`LONG_PRESS_MS`] 当
//! 「向下」；另外定时注入一串自动演示按键，没人操作时界面也会自己走一遍。

#![no_std]
#![no_main]

mod app;
mod display;

use defmt::info;
use embassy_executor::Spawner;
use embedded_graphics_core::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    time::Instant,
};
use kk_ui::{DrawMode, Frame, HEIGHT, Input as UiInput, KEY_DOWN, KEY_OK, KkUi, WIDTH};
use panic_rtt_target as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// 1 bit 画布的放大倍数。
const SCALE: i32 = 2;
/// 屏幕横向逻辑尺寸（Landscape）。
const SCREEN_WIDTH: i32 = 320;
/// 屏幕纵向逻辑尺寸（Landscape）。
const SCREEN_HEIGHT: i32 = 240;
/// 画布左上角，居中摆放。
const ORIGIN_X: i32 = (SCREEN_WIDTH - WIDTH as i32 * SCALE) / 2;
/// 画布顶部，居中摆放。
const ORIGIN_Y: i32 = (SCREEN_HEIGHT - HEIGHT as i32 * SCALE) / 2;
/// BOOT 按住超过该时长按「向下」处理。
const LONG_PRESS_MS: u32 = 700;
/// 按键消抖时间。
const DEBOUNCE_MS: u32 = 15;
/// 一次注入按键的保持时长，必须大于 KK_UI 的 20 ms 消抖。
const KEY_HOLD_MS: u32 = 60;
/// 两次自动演示按键的间隔。
const AUTOPLAY_INTERVAL_MS: u32 = 2500;
/// 主循环节拍。
const TICK_MS: u32 = 5;

/// 自动演示脚本，按键轮流注入，让界面自己走遍各页与各类弹窗。
const AUTOPLAY_KEYS: [u8; 16] = [
    KEY_DOWN, KEY_DOWN, KEY_OK, KEY_DOWN, KEY_OK, KEY_DOWN, KEY_DOWN, KEY_OK, KEY_DOWN, KEY_DOWN,
    KEY_OK, KEY_OK, KEY_DOWN, KEY_OK, KEY_OK, KEY_DOWN,
];

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

/// 把 1 bit 画布按 2 倍放大居中刷到屏上：每条逻辑行送两条物理行。
fn blit(display: &mut display::Display, frame: &Frame) {
    for row in 0..HEIGHT {
        let area = Rectangle::new(
            Point::new(ORIGIN_X, ORIGIN_Y + row as i32 * SCALE),
            Size::new((WIDTH as u32) * SCALE as u32, SCALE as u32),
        );
        let colors = (0..SCALE)
            .flat_map(|_| (0..WIDTH).flat_map(move |x| [color(frame, x, row); SCALE as usize]));
        let _ = display.fill_contiguous(&area, colors);
    }
}

fn color(frame: &Frame, x: i16, y: i16) -> Rgb565 {
    if frame.pixel(x, y) {
        Rgb565::WHITE
    } else {
        Rgb565::BLACK
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

    let mut ui = KkUi::new(&app::APP, app::Wave::new());
    let now = 0u32;
    let status = ui.init();
    info!("kk_ui init: {}", status.as_str());
    app::reset(&mut ui, now);

    let mut button = Button::new();
    let mut injector = KeyInjector::new();
    let mut next_autoplay = AUTOPLAY_INTERVAL_MS;
    let mut script_step = 0usize;
    let mut last_log = 0u32;
    let mut frames = 0u32;

    loop {
        let now = Instant::now().duration_since_epoch().as_millis() as u32;

        if now.wrapping_sub(next_autoplay) as i32 >= 0 {
            next_autoplay = now.wrapping_add(AUTOPLAY_INTERVAL_MS);
            injector.pulse(now, AUTOPLAY_KEYS[script_step % AUTOPLAY_KEYS.len()]);
            script_step += 1;
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
        if ui.take_frame_ready() {
            blit(&mut display, ui.frame());
            if frames == 0 {
                info!("first frame flushed");
            }
            frames += 1;
        }
        if now.wrapping_sub(last_log) >= 5000 {
            last_log = now;
            info!("t={}ms page={} frames={}", now, ui.current_page(), frames);
        }
        delay.delay_millis(TICK_MS);
    }
}

/// 让编译器确认画布绘制模式的导入仍属有意保留。
#[allow(dead_code)]
fn _keep_draw_mode(_: DrawMode) {}
