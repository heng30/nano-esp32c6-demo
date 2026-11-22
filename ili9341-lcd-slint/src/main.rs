#![no_std]
#![no_main]

extern crate alloc;

use defmt::info;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    timer::timg::TimerGroup,
};
use panic_rtt_target as _;
use slint::platform::software_renderer::Rgb565Pixel;

mod display;
mod effects;
mod platform;

slint::include_modules!();

esp_bootloader_esp_idf::esp_app_desc!();

/// Slint 运行时的堆：每帧一张 320×240 的 RGB565 位图（150 KB）也在这里分配。
const HEAP_SIZE: usize = 256 * 1024;
/// 帧间隔：并口满屏一次约 15 ms，留点余量给 Slint 的软件渲染。
const FRAME: Duration = Duration::from_millis(20);

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();
    esp_alloc::heap_allocator!(size: HEAP_SIZE);

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

    let window = platform::init();

    let ui = MainWindow::new().expect("slint ui create failed");
    ui.window().set_size(slint::PhysicalSize::new(
        platform::WIDTH as u32,
        platform::HEIGHT as u32,
    ));
    info!("slint ui ready {}x{}", platform::WIDTH, platform::HEIGHT);

    let mut effects = effects::Effects::new(ui);

    let mut pixels = [Rgb565Pixel(0); platform::WIDTH];
    let mut raw = [0u16; platform::WIDTH];
    let mut rows = 0usize;
    let mut frame_logged = false;

    loop {
        effects.tick();

        slint::platform::update_timers_and_animations();

        window.draw_if_needed(|renderer| {
            renderer.render_by_line(platform::LineWriter {
                display: &mut display,
                pixels: &mut pixels,
                raw: &mut raw,
                rows: &mut rows,
            });
        });

        if !frame_logged && rows > 0 {
            frame_logged = true;
            info!("first frame flushed {} lines", rows);
        }

        Timer::after(FRAME).await;
    }
}
