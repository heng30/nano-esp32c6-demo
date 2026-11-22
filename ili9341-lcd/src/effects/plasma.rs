//! 逐像素算的等离子波。

use defmt::info;
use embassy_time::{Duration, Timer};

use crate::{
    display::Display,
    effects::{DISPLAY_H, DISPLAY_W},
};

/// 每帧相位的步进。
const SPEED: u16 = 3;
/// 帧数。
const FRAMES: u16 = 300;
/// 每帧等待时长。
const FRAME_DELAY: Duration = Duration::from_millis(10);

/// 等离子波，全屏逐像素计算，不需要帧缓冲。
pub struct Plasma;

impl Plasma {
    pub fn new() -> Self {
        Self
    }

    pub async fn run(&mut self, display: &mut Display) {
        info!("effect: plasma");

        for frame in 0..FRAMES {
            let t = (frame * SPEED) as u8;
            let pixels = (0..u32::from(DISPLAY_W) * u32::from(DISPLAY_H)).map(move |i| {
                let x = (i % u32::from(DISPLAY_W)) as u8;
                let y = (i / u32::from(DISPLAY_W)) as u8;
                let r = tri5(x.wrapping_mul(3).wrapping_add(t));
                let g = tri5(
                    y.wrapping_mul(2)
                        .wrapping_add(x.wrapping_mul(2))
                        .wrapping_sub(t),
                );
                let b = tri5(
                    x.wrapping_add(y.wrapping_mul(3))
                        .wrapping_add(t.wrapping_mul(2)),
                );
                (r << 11) | (g << 5) | b
            });
            display
                .draw_raw_iter(0, 0, DISPLAY_W - 1, DISPLAY_H - 1, pixels)
                .expect("plasma failed");
            Timer::after(FRAME_DELAY).await;
        }
    }
}

/// 三角波：相位 u8 映射到 5 位色深，避免逐像素算浮点三角函数。
const fn tri5(phase: u8) -> u16 {
    let v = if phase < 128 { phase } else { 255 - phase };
    (v >> 2) as u16
}
