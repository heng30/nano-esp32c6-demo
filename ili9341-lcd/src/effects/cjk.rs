//! GB2312 汉字逐字出现。

use defmt::info;
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    pixelcolor::{Rgb565, raw::RawU16},
    prelude::*,
};
use static_cell::StaticCell;

use crate::{
    display::Display,
    effects::{DISPLAY_H, DISPLAY_W},
    font_cjk,
};

/// 字形的放大倍数：16 px 字形放到 32 px。
const SCALE: usize = 2;
/// 汉字之间的间距（像素）。
const GAP: usize = 2;
/// 显示的文字，全部落在 GB2312 汉字区。
const TEXT: &str = "中文点阵显示已就绪";
/// 逐字出现的间隔。
const STEP: Duration = Duration::from_millis(150);
/// 整句显示完后的停留时间。
const HOLD: Duration = Duration::from_millis(2000);
/// 文字带高度，等于一个放大后的字形高度。
const BAND_H: u16 = (font_cjk::GLYPH_SIZE * SCALE) as u16;

/// 文字带的渲染缓冲。
static BAND: StaticCell<[u16; DISPLAY_W as usize * BAND_H as usize]> = StaticCell::new();

/// 中文标语，逐字出现后短暂停留。
pub struct Cjk {
    /// 文字带缓冲。
    band: &'static mut [u16],
}

impl Cjk {
    pub fn new() -> Self {
        Self {
            band: BAND.init([0; DISPLAY_W as usize * BAND_H as usize]),
        }
    }

    pub async fn run(&mut self, display: &mut Display) {
        info!("effect: cjk text");

        let top = (DISPLAY_H - BAND_H) / 2;
        let bg = RawU16::from(Rgb565::BLACK).into_inner();
        let fg = RawU16::from(Rgb565::WHITE).into_inner();

        display.clear(Rgb565::BLACK).expect("clear failed");

        for shown in 1..=TEXT.chars().count() {
            self.band.fill(bg);
            render(self.band, shown, fg);
            display
                .draw_raw_slice(0, top, DISPLAY_W - 1, top + BAND_H - 1, self.band)
                .expect("draw failed");
            Timer::after(STEP).await;
        }

        Timer::after(HOLD).await;
    }
}

/// 把前 `shown` 个汉字画进文字带：字形像素放大 `SCALE` 倍，只写点亮的部分。
fn render(band: &mut [u16], shown: usize, fg: u16) {
    let advance = font_cjk::GLYPH_SIZE * SCALE + GAP;
    let total = TEXT.chars().count() * advance - GAP;
    let mut x = (DISPLAY_W as usize - total) / 2;

    for (i, ch) in TEXT.chars().enumerate() {
        if i >= shown {
            break;
        }
        if let Some(glyph) = font_cjk::glyph(ch) {
            for row in 0..font_cjk::GLYPH_SIZE {
                let bits = u16::from_be_bytes([glyph[row * 2], glyph[row * 2 + 1]]);
                for col in 0..font_cjk::GLYPH_SIZE {
                    if bits & (1 << (15 - col)) == 0 {
                        continue;
                    }
                    for dy in 0..SCALE {
                        for dx in 0..SCALE {
                            let px = x + col * SCALE + dx;
                            let py = row * SCALE + dy;
                            band[py * DISPLAY_W as usize + px] = fg;
                        }
                    }
                }
            }
        }
        x += advance;
    }
}
