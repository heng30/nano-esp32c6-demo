//! 滚动字幕 + 底部进度条。

use defmt::info;
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    draw_target::DrawTarget,
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};
use static_cell::StaticCell;

use crate::{
    display::Display,
    effects::{DISPLAY_H, DISPLAY_W},
};

/// 字幕文本，只用 ASCII（8×8 字体只覆盖 ASCII）。
const TEXT: &[u8] = b"ESP32-C6 + ILI9341  8080-16 PARALLEL BUS  NO LCD CONTROLLER  ";
/// 字形放大倍数。
const SCALE: usize = 3;
/// 一个字符占的水平像素。
const ADVANCE: i32 = 8 * SCALE as i32;
/// 字幕带高度。
const BAND_H: u16 = 8 * SCALE as u16;
/// 字幕前景色（RGB565 白）。
const FG: u16 = 0xffff;
/// 字幕背景色（RGB565 黑）。
const BG: u16 = 0x0000;
/// 每帧左移的像素。
const SPEED: i32 = 5;
/// 帧数，跑完一整轮文本。
const FRAMES: u16 = 260;
/// 每帧等待时长。
const FRAME_DELAY: Duration = Duration::from_millis(30);

/// 进度条宽度（像素）。
const BAR_W: u16 = 260;
/// 进度条左上角。
const BAR_ORIGIN: (i32, i32) = (30, 200);
/// 进度条高度（像素）。
const BAR_H: u32 = 12;

/// 字幕带的渲染缓冲。
static BAND: StaticCell<[u16; DISPLAY_W as usize * BAND_H as usize]> = StaticCell::new();

/// 滚动的英文标语，底下一个进度条。
pub struct Scroll {
    /// 字幕带缓冲。
    band: &'static mut [u16],
}

impl Scroll {
    pub fn new() -> Self {
        Self {
            band: BAND.init([0; DISPLAY_W as usize * BAND_H as usize]),
        }
    }

    pub async fn run(&mut self, display: &mut Display) {
        info!("effect: scroll text");

        let top = (DISPLAY_H - BAND_H) / 2;

        display.clear(Rgb565::BLACK).expect("clear failed");

        for frame in 0..FRAMES {
            render_band(self.band, frame as i32 * SPEED);
            display
                .draw_raw_slice(0, top, DISPLAY_W - 1, top + BAND_H - 1, self.band)
                .expect("text failed");

            let done = (frame as u32 + 1) * u32::from(BAR_W) / u32::from(FRAMES);
            Rectangle::new(
                Point::new(BAR_ORIGIN.0, BAR_ORIGIN.1),
                Size::new(done, BAR_H),
            )
            .into_styled(PrimitiveStyle::with_fill(Rgb565::YELLOW))
            .draw(display)
            .expect("bar failed");
            // 用黑矩形把上一帧多出来的部分盖掉
            Rectangle::new(
                Point::new(BAR_ORIGIN.0 + done as i32, BAR_ORIGIN.1),
                Size::new(u32::from(BAR_W) - done, BAR_H),
            )
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(display)
            .expect("bar failed");

            Timer::after(FRAME_DELAY).await;
        }
    }
}

/// 把字幕的一帧渲染进缓冲：每列只查一次字形，再填满整列。
fn render_band(band: &mut [u16], shift: i32) {
    let text_len = TEXT.len() as i32;

    for x in 0..DISPLAY_W as usize {
        let px = (x as i32 + shift).rem_euclid(ADVANCE * text_len);
        let glyph = font8x8::legacy::BASIC_LEGACY[TEXT[(px / ADVANCE) as usize] as usize];
        let col = (px % ADVANCE) as usize / SCALE;

        for (row, bits) in glyph.iter().enumerate() {
            let color = if (bits >> col) & 1 == 1 { FG } else { BG };
            for dy in 0..SCALE {
                band[(row * SCALE + dy) * DISPLAY_W as usize + x] = color;
            }
        }
    }
}
