//! 两条相位相反的正弦波。

use defmt::info;
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    pixelcolor::{Rgb565, raw::RawU16},
    prelude::*,
};
use static_cell::StaticCell;

use crate::{
    display::Display,
    effects::{DISPLAY_H, DISPLAY_W, sin_q10},
};

/// 波形区高度（像素），也是局部帧缓冲的高度。
const BAND_H: u16 = 120;
/// 振幅（像素）。
const AMP: i32 = 50;
/// 每列水平像素对应的角度增量（度）。
const STEP: u16 = 5;
/// 每帧相位增量（度）。
const PHASE_STEP: u16 = 8;
/// 帧数。
const FRAMES: u16 = 300;
/// 每帧等待时长。
const FRAME_DELAY: Duration = Duration::from_millis(20);

/// 波形区的局部帧缓冲。
static BAND: StaticCell<[u16; DISPLAY_W as usize * BAND_H as usize]> = StaticCell::new();

/// 两条相位相反的正弦曲线，青上黄下。
pub struct Wave {
    /// 波形区的重绘缓冲。
    band: &'static mut [u16],
}

impl Wave {
    pub fn new() -> Self {
        Self {
            band: BAND.init([0; DISPLAY_W as usize * BAND_H as usize]),
        }
    }

    /// 每帧整块重算波形区再一次送出；相邻列之间用竖线插值，曲线不会断成点。
    pub async fn run(&mut self, display: &mut Display) {
        info!("effect: sine wave");

        let top = (DISPLAY_H - BAND_H) / 2;
        let mid = (BAND_H / 2) as i32;
        let width = DISPLAY_W as usize;
        let bg = RawU16::from(Rgb565::BLACK).into_inner();
        let up_color = RawU16::from(Rgb565::CYAN).into_inner();
        let down_color = RawU16::from(Rgb565::YELLOW).into_inner();

        display.clear(Rgb565::BLACK).expect("clear failed");

        for frame in 0..FRAMES {
            let phase = frame * PHASE_STEP;
            self.band.fill(bg);

            let mut prev_up = mid;
            let mut prev_down = mid;
            for x in 0..width {
                let amp = sin_q10(phase + x as u16 * STEP) * AMP / 1024;
                let up = mid + amp;
                let down = mid - amp;

                for y in prev_up.min(up)..=prev_up.max(up) {
                    self.band[y as usize * width + x] = up_color;
                }
                for y in prev_down.min(down)..=prev_down.max(down) {
                    self.band[y as usize * width + x] = down_color;
                }
                prev_up = up;
                prev_down = down;
            }

            display
                .draw_raw_slice(0, top, DISPLAY_W - 1, top + BAND_H - 1, self.band)
                .expect("draw failed");
            Timer::after(FRAME_DELAY).await;
        }
    }
}
