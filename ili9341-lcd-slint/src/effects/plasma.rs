//! 逐像素算的等离子波，用三角波代替 sin。

use slint::platform::software_renderer::Rgb565Pixel;

use super::{Canvas, HEIGHT, WIDTH};

/// 每帧相位的步进。
const SPEED: u32 = 3;

/// 等离子波，全屏逐像素计算。
pub struct Plasma;

impl Canvas for Plasma {
    fn draw(&mut self, px: &mut [Rgb565Pixel], t_ms: u32) {
        let t = (t_ms * SPEED / 16) as u8;

        for y in 0..HEIGHT {
            let yu = y as u8;
            for x in 0..WIDTH {
                let xu = x as u8;
                let r = tri5(xu.wrapping_mul(3).wrapping_add(t));
                let g = tri5(
                    yu.wrapping_mul(2)
                        .wrapping_add(xu.wrapping_mul(2))
                        .wrapping_sub(t),
                );
                let b = tri5(
                    xu.wrapping_add(yu.wrapping_mul(3))
                        .wrapping_add(t.wrapping_mul(2)),
                );
                px[y * WIDTH + x] = Rgb565Pixel((r << 11) | (g << 5) | b);
            }
        }
    }
}

/// 三角波：相位 u8 映射到 5 位色深，避免逐像素算浮点三角函数。
const fn tri5(phase: u8) -> u16 {
    let v = if phase < 128 { phase } else { 255 - phase };
    (v >> 2) as u16
}
