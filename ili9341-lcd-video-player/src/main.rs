#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_time::{Instant, Timer};
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    timer::timg::TimerGroup,
};
use font8x8::legacy::BASIC_LEGACY;
use panic_rtt_target as _;
use zune_jpeg::{JpegDecoder, zune_core::bytestream::ZCursor};

mod avi;
mod display;

/// 内嵌的测试视频：160×120、25 fps、218 帧、MJPEG（无音轨），由 `assets/videos/` 里
/// 320×240 的原片缩放重编码而来，写屏时 2× 放大铺满整屏。
const VIDEO: &[u8] = include_bytes!("../../assets/videos/test-160x120-without-audio.avi");
/// 堆：一帧解码 RGB 160×120×3 = 57.6 KB + 整帧 RGB565 153.6 KB。
const HEAP_SIZE: usize = 256 * 1024;
/// 解码出来的画面宽（像素）。
const FRAME_W: usize = 160;
/// 帧率叠层的字符格数，每格 8×8 像素。
const HUD_COLS: usize = 8;
/// 帧率叠层宽（像素）。
const HUD_W: usize = HUD_COLS * 8;
/// 帧率读数每这么久刷新一次。
const FPS_WINDOW_MS: u64 = 1000;

esp_bootloader_esp_idf::esp_app_desc!();

/// 解一帧 MJPEG，返回 (RGB 像素, 解码微秒数)。
fn decode_frame(jpeg: &[u8]) -> (alloc::vec::Vec<u8>, u64) {
    let started = Instant::now();
    // 默认输出就是 RGB，与 zune-jpeg 的 `DecoderOptions::default()` 一致，不用额外配置
    let rgb = JpegDecoder::new(ZCursor::new(jpeg))
        .decode()
        .expect("mjpeg frame decode failed");
    (rgb, started.elapsed().as_micros())
}

/// 把一个源行（160 像素）转成 RGB565 并横向 2× 放大成整屏宽的一行。
fn expand_row(rgb: &[u8], src_row: usize, out: &mut [u16; display::WIDTH]) {
    for (x, cell) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
        let p = &rgb[(src_row * FRAME_W + x) * 3..][..3];
        let pixel =
            (u16::from(p[0] & 0xf8) << 8) | (u16::from(p[1] & 0xfc) << 3) | u16::from(p[2] >> 3);
        *cell = [pixel, pixel];
    }
}

/// 在右上角画 "NN fps"：8×8 点阵、黑底白字，直接写进帧缓冲。
fn draw_hud(frame: &mut [u16], fps: u32) {
    let mut text = [b' '; HUD_COLS];
    text[7] = b's';
    text[6] = b'p';
    text[5] = b'f';
    let mut value = fps.min(999);
    let mut at = 4;
    loop {
        text[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 || at == 0 {
            break;
        }
        at -= 1;
    }

    let left = display::WIDTH - HUD_W;
    for x in 0..HUD_W {
        let glyph = BASIC_LEGACY[text[x / 8] as usize];
        for (y, row) in glyph.iter().enumerate() {
            frame[y * display::WIDTH + left + x] =
                if (row >> (x % 8)) & 1 == 1 { 0xffff } else { 0 };
        }
    }
}

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();
    esp_alloc::heap_allocator!(size: HEAP_SIZE);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let delay = Delay::new();

    // D0–D15 依次是 GPIO0–7、GPIO8、GPIO10、GPIO11、GPIO18–22，WR 走 GPIO23，DC 走 GPIO15
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
    let mut display = display::Display::new(
        data,
        Output::new(peripherals.GPIO15, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO23, Level::High, OutputConfig::default()),
        &delay,
    );

    let video = avi::Avi::parse(VIDEO).expect("embedded avi is malformed");
    if (video.width(), video.height()) != (FRAME_W as u16, display::HEIGHT as u16 / 2) {
        warn!(
            "video is {}x{}, player expects {}x{}",
            video.width(),
            video.height(),
            FRAME_W,
            display::HEIGHT / 2
        );
    }
    let frame_delay = video.frame_delay();
    info!(
        "video {}x{}, {} frames, {} us/frame",
        video.width(),
        video.height(),
        video.frames().count(),
        frame_delay.as_micros()
    );

    let mut line = [0u16; display::WIDTH];
    let mut frame = vec![0u16; display::WIDTH * display::HEIGHT];
    let mut fps = 0u32;
    let mut window = Instant::now();
    let mut counted = 0u32;

    loop {
        let started = Instant::now();
        let mut played = 0u32;
        let (mut decode_us, mut draw_us) = (0u64, 0u64);

        for jpeg in video.frames() {
            let frame_started = Instant::now();
            let (rgb, decode) = decode_frame(jpeg);
            decode_us += decode;

            let draw_started = Instant::now();
            // 整帧 2× 放大进帧缓冲（纵向 2× 用同一行写两遍）
            for src_row in 0..display::HEIGHT / 2 {
                expand_row(&rgb, src_row, &mut line);
                frame[src_row * 2 * display::WIDTH..][..display::WIDTH].copy_from_slice(&line);
                frame[(src_row * 2 + 1) * display::WIDTH..][..display::WIDTH]
                    .copy_from_slice(&line);
            }
            draw_hud(&mut frame, fps);
            display.draw_frame(&frame);
            draw_us += draw_started.elapsed().as_micros();

            played += 1;
            counted += 1;
            let window_ms = window.elapsed().as_millis();
            if window_ms >= FPS_WINDOW_MS {
                fps = counted * 1000 / u32::try_from(window_ms).unwrap_or(u32::MAX).max(1);
                counted = 0;
                window = Instant::now();
            }

            // 解码比帧间隔慢时不睡，宁可变慢也不丢帧
            if let Some(rest) = frame_delay.checked_sub(frame_started.elapsed()) {
                Timer::after(rest).await;
            }
        }

        let elapsed = started.elapsed();
        info!(
            "played {} frames in {} ms ({} fps): decode {} us, draw {} us per frame",
            played,
            elapsed.as_millis(),
            u64::from(played) * 1000 / elapsed.as_millis().max(1),
            decode_us / u64::from(played),
            draw_us / u64::from(played)
        );
    }
}
