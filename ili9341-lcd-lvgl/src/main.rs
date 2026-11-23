#![no_std]
#![no_main]

//! 用 LVGL 9 画音乐播放器页：`lv_image` 旋转唱片、`lv_line` 唱臂、`lv_bar` 进度、`lv_label` 歌词。
//!
//! LVGL 的 C 源码由 `lvgl-rust-sys` 在构建期用交叉 C 编译器编进来，Rust 侧走 `lvgl-rust` 的安全
//! 包装：控件只留 4 字节句柄，每帧在一个 session 里重新解析。只有 flush 的字节流转换和唱臂的
//! 坐标数组两处碰裸指针。

use core::sync::atomic::{AtomicBool, Ordering};

use defmt::info;
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    timer::timg::TimerGroup,
};
use lvgl_rust::{
    Display, DisplayCell, DrawBuf, DrawBuffers, Flush, FlushRequest, Lvgl, LvglAccess, RenderMode,
    ScreenAnim, tick_inc,
};
use panic_rtt_target as _;

mod display;
mod home;
mod music;

esp_bootloader_esp_idf::esp_app_desc!();

/// 帧间隔。
const FRAME: Duration = Duration::from_millis(20);
/// 音乐页走完一轮（`music::CYCLE_MS`）之后，插播控制面板的时长。
const HOME_MS: u32 = 30_000;
/// 绘制缓冲的行数：320×40 像素，整帧 150 KB 在 512 KB SRAM 里放不下。
const DRAW_ROWS: usize = 40;
/// 绘制缓冲字节数（RGB565）。
const DRAW_BUF_BYTES: usize = music::WIDTH as usize * 2 * DRAW_ROWS;

/// 屏驱动：所有状态都在这里，LVGL 只持有它的引用。
static PANEL: DisplayCell<Panel> = DisplayCell::new();
/// LVGL 的绘制缓冲。
static DRAW_BUF: DrawBuf<DRAW_BUF_BYTES> = DrawBuf::new();
/// 首帧是否已经刷过，只用来打一条日志确认屏真的在画。
static FIRST_FLUSH: AtomicBool = AtomicBool::new(false);

/// 证明「此刻只有本任务在碰 LVGL」的令牌。
///
/// LVGL 的全部调用都在 `main` 这一个任务里，flush 也是 `timer_handler` 同步回调过来的。
/// 不用 `critical_section` 互斥：那会把每帧约 20 ms 的刷新期间中断全关掉。
struct Token;

// SAFETY: 令牌只在 `main` 的帧循环里就地创建，LVGL 的并发入口只有这一个任务。
unsafe impl LvglAccess for Token {}

/// 并口屏 + 一行像素的转换缓冲。
struct Panel {
    /// ili9341 驱动，已初始化成 320×240 横屏。
    bus: display::Display,
    /// 一行 RGB565：LVGL 给的是字节流，并口一次要一个 u16。
    row: [u16; music::WIDTH as usize],
}

impl Flush for Panel {
    fn flush(&mut self, mut req: FlushRequest<'_>) {
        let area = req.area();
        let stride = req.stride();
        let pixels = req.pixels();
        let width = (area.x2 - area.x1 + 1) as usize;

        if !FIRST_FLUSH.swap(true, Ordering::Relaxed) {
            info!(
                "first flush {}x{} at ({},{})",
                width,
                area.y2 - area.y1 + 1,
                area.x1,
                area.y1
            );
        }

        for dy in 0..=(area.y2 - area.y1) as usize {
            let start = dy * stride;
            for (i, px) in self.row[..width].iter_mut().enumerate() {
                *px = u16::from_le_bytes([pixels[start + i * 2], pixels[start + i * 2 + 1]]);
            }
            let y = (area.y1 as usize + dy) as u16;
            self.bus
                .draw_raw_slice(area.x1 as u16, y, area.x2 as u16, y, &self.row[..width])
                .ok();
        }
    }
}

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
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

    let display = display::new(
        data,
        Output::new(peripherals.GPIO15, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO23, Level::High, OutputConfig::default()),
        &mut delay,
    );

    let mut token = Token;
    let mut lv = lvgl_rust::init(&mut token).expect("lvgl init failed");

    let _display = Display::create(
        &mut lv,
        &PANEL,
        Panel {
            bus: display,
            row: [0; music::WIDTH as usize],
        },
        music::WIDTH,
        music::HEIGHT,
        DrawBuffers::single(&DRAW_BUF),
        RenderMode::Partial,
    )
    .expect("display create failed");

    let home = home::Home::build(&mut lv).expect("home build failed");
    let mut music = music::Music::build(&mut lv).expect("music build failed");
    // lv.load_screen(music.screen, ScreenAnim::NONE)
    //     .expect("load screen failed");

    info!("lvgl ready {}x{}", music::WIDTH, music::HEIGHT);
    drop(lv);

    let started = Instant::now();
    let mut showing_home = false;
    let mut need_reload_screen = true;

    loop {
        let mut token = Token;
        let mut lv = Lvgl::enter(&mut token).expect("lvgl enter failed");
        tick_inc(FRAME.as_millis() as u32);

        let elapsed = (Instant::now() - started).as_millis() as u32;

        // 一轮音乐播完就插播 30 秒控制面板，再切回音乐页
        let want_home = elapsed % (music::CYCLE_MS + HOME_MS) >= music::CYCLE_MS;
        if showing_home != want_home {
            need_reload_screen = true;
        }

        if need_reload_screen {
            let screen = if want_home { home.screen } else { music.screen };
            lv.load_screen(screen, ScreenAnim::NONE)
                .expect("load screen failed");
            info!("page: {}", if want_home { "home" } else { "music" });
            showing_home = want_home;
            need_reload_screen = false;
        }

        music.tick(&mut lv, elapsed).expect("music tick failed");
        lv.timer_handler();
        drop(lv);

        Timer::after(FRAME).await;
    }
}
