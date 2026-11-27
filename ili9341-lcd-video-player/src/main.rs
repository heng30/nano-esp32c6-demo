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
/// 解码器堆：一帧 RGB 160×120×3 = 57.6 KB，加一帧 RGB565 指纹 38.4 KB。
const HEAP_SIZE: usize = 192 * 1024;
/// 屏宽（像素），与视频 2× 放大后同尺寸。
const SCREEN_W: u16 = 320;
/// 屏高（像素）。
const SCREEN_H: u16 = 240;
/// 解码出来的画面宽（像素）。
const FRAME_W: usize = 160;
/// 解码出来的画面高（像素）。
const FRAME_H: usize = 120;
/// 源块边长（像素）。
const SRC_BLOCK: usize = 8;
/// 写到屏上的放大倍数。
const SCALE: usize = 2;
/// 屏上块边长（像素）。
const DST_BLOCK: usize = SRC_BLOCK * SCALE;
/// 块网格的列数。
const BLOCK_COLS: usize = FRAME_W / SRC_BLOCK;
/// 块网格的行数。
const BLOCK_ROWS: usize = FRAME_H / SRC_BLOCK;
/// 一帧的块数（20×15 = 300）。
const BLOCKS: usize = BLOCK_COLS * BLOCK_ROWS;
/// 变化判定的死区掩码：R 低 1 位、G 低 2 位、B 低 1 位不参与比较，压掉 JPEG 的重编码噪声。
const DIFF_MASK: u16 = 0xf79e;
// 块网格必须正好铺满整屏，改分辨率时这里会先报警
const _: () = assert!(
    BLOCK_COLS * DST_BLOCK == SCREEN_W as usize && BLOCK_ROWS * DST_BLOCK == SCREEN_H as usize
);
/// 帧率叠层的字符格数，每格 8×8 像素。
const HUD_COLS: usize = 8;
/// 帧率叠层宽（像素）。
const HUD_W: usize = HUD_COLS * 8;
/// 帧率叠层高（像素）。
const HUD_H: usize = 8;
/// 帧率读数每这么久刷新一次。
const FPS_WINDOW_MS: u64 = 1000;

esp_bootloader_esp_idf::esp_app_desc!();

/// 解一帧 → 与上一帧逐块比对 → 只把变了的 8×8 块 2× 放大写到屏上的 16×16 块。
///
/// 返回 (解码, 写屏) 的微秒数。`prev` 存上一帧的 RGB565，长度 `BLOCKS * SRC_BLOCK²`。
fn play_frame(
    display: &mut display::Display,
    prev: &mut [u16],
    jpeg: &[u8],
) -> (u64, u64, u64, u32) {
    let started = Instant::now();
    // 默认输出就是 RGB，与 zune-jpeg 的 `DecoderOptions::default()` 一致，不用额外配置
    let rgb = JpegDecoder::new(ZCursor::new(jpeg))
        .decode()
        .expect("mjpeg frame decode failed");
    let decode_us = started.elapsed().as_micros();
    let started = Instant::now();

    let mut dst = [0u16; DST_BLOCK * DST_BLOCK];
    let diff_started = Instant::now();
    let (mut prep_us, mut drawn) = (0, 0u32);

    for row in 0..BLOCK_ROWS {
        for col in 0..BLOCK_COLS {
            let at = row * BLOCK_COLS + col;
            let source = &mut prev[at * SRC_BLOCK * SRC_BLOCK..][..SRC_BLOCK * SRC_BLOCK];

            let mut changed = false;
            for y in 0..SRC_BLOCK {
                let line = ((row * SRC_BLOCK + y) * FRAME_W + col * SRC_BLOCK) * 3;
                for (x, slot) in source[y * SRC_BLOCK..][..SRC_BLOCK].iter_mut().enumerate() {
                    let p = &rgb[line + x * 3..][..3];
                    let pixel = (u16::from(p[0] & 0xf8) << 8)
                        | (u16::from(p[1] & 0xfc) << 3)
                        | u16::from(p[2] >> 3);
                    changed |= (*slot & DIFF_MASK) != (pixel & DIFF_MASK);
                    *slot = pixel;
                }
            }
            if !changed {
                continue;
            }

            let prep = Instant::now();
            // 2×2 放大：每个源像素铺成一个 2×2 的色块
            for y in 0..SRC_BLOCK {
                for x in 0..SRC_BLOCK {
                    let value = source[y * SRC_BLOCK + x];
                    for dy in 0..SCALE {
                        let line = (y * SCALE + dy) * DST_BLOCK + x * SCALE;
                        dst[line..line + SCALE].fill(value);
                    }
                }
            }

            prep_us += prep.elapsed().as_micros();
            drawn += 1;
            let x0 = (col * DST_BLOCK) as u16;
            let y0 = (row * DST_BLOCK) as u16;
            display
                .draw_raw_slice(
                    x0,
                    y0,
                    x0 + DST_BLOCK as u16 - 1,
                    y0 + DST_BLOCK as u16 - 1,
                    &dst,
                )
                .expect("block draw failed");
        }
    }

    let draw_us = started.elapsed().as_micros();
    (
        decode_us,
        draw_us,
        prep_us + diff_started.elapsed().as_micros(),
        drawn,
    )
}

/// 在右上角画 "NN fps"：8×8 点阵、黑底白字，整帧重刷时会被视频盖掉，所以每帧都要重画。
fn draw_hud(display: &mut display::Display, fps: u32, buf: &mut [u16; HUD_W * HUD_H]) {
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

    for x in 0..HUD_W {
        let glyph = BASIC_LEGACY[text[x / 8] as usize];
        for (y, row) in glyph.iter().enumerate() {
            buf[y * HUD_W + x] = if (row >> (x % 8)) & 1 == 1 { 0xffff } else { 0 };
        }
    }

    display
        .draw_raw_slice(
            SCREEN_W - HUD_W as u16,
            0,
            SCREEN_W - 1,
            HUD_H as u16 - 1,
            buf,
        )
        .expect("fps overlay failed");
}

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

    let video = avi::Avi::parse(VIDEO).expect("embedded avi is malformed");
    if (video.width(), video.height()) != (FRAME_W as u16, FRAME_H as u16) {
        warn!(
            "video is {}x{}, player expects {}x{}",
            video.width(),
            video.height(),
            FRAME_W,
            FRAME_H
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

    let mut prev = vec![0u16; BLOCKS * SRC_BLOCK * SRC_BLOCK];
    let mut hud = [0u16; HUD_W * HUD_H];
    let mut fps = 0u32;
    let mut window = Instant::now();
    let mut counted = 0u32;

    loop {
        let started = Instant::now();
        let mut played = 0u32;
        let (mut decode_us, mut draw_us) = (0u64, 0u64);
        let (mut prep_us, mut drawn) = (0u64, 0u64);

        for frame in video.frames() {
            let frame_started = Instant::now();
            let (decode, draw, prep, blocks) = play_frame(&mut display, &mut prev, frame);
            decode_us += decode;
            draw_us += draw;
            prep_us += prep;
            drawn += u64::from(blocks);
            played += 1;
            counted += 1;

            let window_ms = window.elapsed().as_millis();
            if window_ms >= FPS_WINDOW_MS {
                fps = counted * 1000 / u32::try_from(window_ms).unwrap_or(u32::MAX).max(1);
                counted = 0;
                window = Instant::now();
            }
            draw_hud(&mut display, fps, &mut hud);

            // 解码比帧间隔慢时不睡，宁可变慢也不丢帧
            if let Some(rest) = frame_delay.checked_sub(frame_started.elapsed()) {
                Timer::after(rest).await;
            }
        }

        let elapsed = started.elapsed();
        info!(
            "played {} frames in {} ms ({} fps): decode {} us, draw {} us, prep {} us, {} blocks/frame",
            played,
            elapsed.as_millis(),
            u64::from(played) * 1000 / elapsed.as_millis().max(1),
            decode_us / u64::from(played),
            draw_us / u64::from(played),
            prep_us / u64::from(played),
            drawn / u64::from(played)
        );
    }
}
