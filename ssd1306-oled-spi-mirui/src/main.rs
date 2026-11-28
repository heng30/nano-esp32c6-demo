//! 在 SSD1306 128×64 OLED（SPI）上用 mirui 画一个音乐播放器页。
//!
//! mirui 渲染进自己的 RGB565 帧缓冲，每帧把缓冲的 flush 回调里压成 1 bit 推给屏；
//! 播放进度、曲名与歌词走 `Signal` + 一个系统，转盘角度与唱针位置在 view 的 paint
//! 里从 systimer 现算——裸机上没有现成的 tick 源，`EspClock` 把 mirui 的进程时钟
//! 接到 `esp_hal::time::Instant`。

#![no_std]
#![no_main]

extern crate alloc;

mod display;
mod music;

use defmt::info;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    spi::master::{Config as SpiConfig, Spi},
    time::{Instant, Rate},
};
use mirui::{
    app::{App, RendererFactory, plugin::Plugin},
    ecs::MonoClock,
    prelude::*,
    render::texture::ColorFormat,
    surface::{Surface, framebuf::FramebufSurface},
};
use panic_rtt_target as _;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

/// 堆大小：mirui 的 ECS、字体光栅缓存与 16 KB 的 RGB565 帧缓冲都从这里出。
const HEAP_SIZE: usize = 160 * 1024;
/// SPI 时钟：SSD1306 上限 10 MHz。
const SPI_FREQ_MHZ: u32 = 8;
/// 统计日志的间隔。
const LOG_INTERVAL_MS: u32 = 5000;

/// 1 bit 影子显存，跟屏一样大，放在 `.bss` 而不是主任务栈上。
static MONO: StaticCell<[u8; display::FRAME_LEN]> = StaticCell::new();

/// 把 mirui 的进程时钟接到 systimer 上，并提供 `MonoClock` 资源。
struct EspClock;

impl<B: Surface, F: RendererFactory<B>> Plugin<B, F> for EspClock {
    fn build(&mut self, app: &mut App<B, F>) {
        mirui::core::time::set_clock(now_ns);
        app.world.insert_resource(MonoClock::from_time_source());
    }
}

/// systimer 读数换算成纳秒，直接当 mirui 的单调时钟。
fn now_ns() -> u64 {
    Instant::now()
        .duration_since_epoch()
        .as_micros()
        .saturating_mul(1000)
}

#[esp_hal::main]
fn main() -> ! {
    rtt_target::rtt_init_defmt!();
    esp_alloc::heap_allocator!(size: HEAP_SIZE);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

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
        MONO.init([0; display::FRAME_LEN]),
    );

    // mirui 只报脏矩形，但整块缓冲始终是完整画面，所以忽略 `area` 整帧重写即可。
    let mut frames = 0u32;
    let mut flush_us = 0u64;
    let mut last_log = 0u32;
    let mut logged_first = false;
    let backend = FramebufSurface::with_format(
        display::WIDTH,
        display::HEIGHT,
        ColorFormat::RGB565,
        move |rgb565: &[u8], _area: mirui::types::PhysicalRect| {
            let started = Instant::now();
            display.push(rgb565);
            flush_us += started.elapsed().as_micros();
            frames += 1;
            if !logged_first {
                logged_first = true;
                info!("first frame pushed");
            }
            let now = Instant::now().duration_since_epoch().as_millis() as u32;
            if now.wrapping_sub(last_log) >= LOG_INTERVAL_MS {
                last_log = now;
                info!(
                    "t={}ms frames={} push={}us",
                    now,
                    frames,
                    flush_us / frames as u64
                );
                flush_us = 0;
                frames = 0;
            }
        },
    );

    let state = Signal::new(music::Readout::default());
    let mut app = App::new(backend);
    app.with_default_widgets()
        .with_widget(music::turntable_view())
        .with_default_systems();
    app.add_plugin(EspClock);
    app.add_resource(music::Playback(state.clone()));
    app.add_system(music::playback_tick::system());

    let root = app.spawn_root().id();
    app.compose(root, music::build_page(state));
    info!("mirui ready {}x{}", display::WIDTH, display::HEIGHT);

    app.run();
    loop {
        core::hint::spin_loop();
    }
}
