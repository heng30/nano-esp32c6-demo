#![no_std]
#![no_main]

//! 用 LVGL 画音乐播放器页：`lv_img` 旋转唱片、`lv_line` 唱臂、`lv_bar` 进度、`lv_label` 歌词。
//!
//! LVGL 的 C 源码由 `lvgl-sys` 在构建期用交叉 C 编译器编进来，Rust 侧只做外设、tick 与 flush。

use core::cell::RefCell;
use core::ptr::{addr_of_mut, null_mut};
use core::sync::atomic::{AtomicBool, Ordering};

use critical_section::Mutex;
use defmt::info;
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    timer::timg::TimerGroup,
};
use lvgl_sys as lv;
use panic_rtt_target as _;

mod display;
mod music;

esp_bootloader_esp_idf::esp_app_desc!();

/// 帧间隔。
const FRAME: Duration = Duration::from_millis(20);
/// LVGL 绘制缓冲按行分块：320×40 像素，整帧 150 KB 在 512 KB SRAM 里放不下。
const DRAW_BUF_PX: usize = music::WIDTH as usize * 40;

/// 并口屏。flush 回调是 C 调过来的，拿不到 `main` 里的所有权，只能放 static。
static DISPLAY: Mutex<RefCell<Option<display::Display>>> = Mutex::new(RefCell::new(None));
/// LVGL 的绘制缓冲。
static mut DRAW_BUF: [u16; DRAW_BUF_PX] = [0; DRAW_BUF_PX];
/// LVGL 的缓冲描述符与显示驱动，`lv_init` 之后才填。
static mut DRAW_BUF_DSC: core::mem::MaybeUninit<lv::_lv_disp_draw_buf_t> = core::mem::MaybeUninit::uninit();
static mut DISP_DRV: core::mem::MaybeUninit<lv::lv_disp_drv_t> = core::mem::MaybeUninit::uninit();
/// 首帧是否已经刷过，只用来打一条日志确认屏真的在画。
static FIRST_FLUSH: AtomicBool = AtomicBool::new(false);

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

    lvgl_init(display);
    info!("lvgl ready {}x{}", music::WIDTH, music::HEIGHT);

    let mut music = music::Music::build();
    let started = Instant::now();

    loop {
        unsafe { lv::lv_tick_inc(FRAME.as_millis() as u32) };
        music.tick((Instant::now() - started).as_millis() as u32);
        unsafe { lv::lv_timer_handler() };
        Timer::after(FRAME).await;
    }
}

/// 初始化 LVGL、注册显示驱动，并把屏的所有权挪进 static。
fn lvgl_init(display: display::Display) {
    critical_section::with(|cs| {
        DISPLAY.borrow_ref_mut(cs).replace(display);
    });

    unsafe {
        lv::lv_init();

        let buf_dsc = addr_of_mut!(DRAW_BUF_DSC).cast::<lv::_lv_disp_draw_buf_t>();
        lv::lv_disp_draw_buf_init(
            buf_dsc,
            addr_of_mut!(DRAW_BUF).cast(),
            null_mut(),
            DRAW_BUF_PX as u32,
        );

        let drv = addr_of_mut!(DISP_DRV).cast::<lv::lv_disp_drv_t>();
        lv::lv_disp_drv_init(drv);
        (*drv).hor_res = music::WIDTH;
        (*drv).ver_res = music::HEIGHT;
        (*drv).draw_buf = buf_dsc;
        (*drv).flush_cb = Some(flush);
        lv::lv_disp_drv_register(drv);
    }
}

/// LVGL 刷一块区域就调一次：逐行写并口，写完告诉 LVGL 这块可以复用了。
unsafe extern "C" fn flush(
    drv: *mut lv::lv_disp_drv_t,
    area: *const lv::lv_area_t,
    px: *mut lv::lv_color16_t,
) {
    unsafe {
        let area = &*area;
        let width = (area.x2 - area.x1 + 1) as usize;
        let mut row = px as *const u16;

        if !FIRST_FLUSH.swap(true, Ordering::Relaxed) {
            info!("first flush {}x{} at ({},{})", width, area.y2 - area.y1 + 1, area.x1, area.y1);
        }

        critical_section::with(|cs| {
            if let Some(display) = DISPLAY.borrow_ref_mut(cs).as_mut() {
                for y in area.y1..=area.y2 {
                    let line = core::slice::from_raw_parts(row, width);
                    display
                        .draw_raw_slice(area.x1 as u16, y as u16, area.x2 as u16, y as u16, line)
                        .expect("flush failed");
                    row = row.add(width);
                }
            }
        });

        lv::lv_disp_flush_ready(drv);
    }
}
