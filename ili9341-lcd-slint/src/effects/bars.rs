//! 动态柱状图：双正弦叠加 + 峰值保持，模拟频谱。

use slint::platform::software_renderer::Rgb565Pixel;

use super::{Canvas, HEIGHT, WIDTH, sin_q10};

/// 柱子数量，必须整除画布宽度。
const BARS: usize = 16;
/// 柱宽（像素）。
const BAR_W: usize = WIDTH / BARS;
/// 柱子最大高度（像素），底部留一点边。
const MAX_H: i32 = HEIGHT as i32 - 8;
/// 每帧峰值回落的高度（像素）。
const PEAK_DECAY: i32 = 2;

/// 动态柱状图。
pub struct Bars {
    /// 各柱当前高度（像素）。
    level: [i32; BARS],
    /// 各柱峰值高度（像素），缓慢回落。
    peak: [i32; BARS],
}

impl Bars {
    pub fn new() -> Self {
        Self {
            level: [0; BARS],
            peak: [0; BARS],
        }
    }
}

impl Canvas for Bars {
    fn draw(&mut self, px: &mut [Rgb565Pixel], t_ms: u32) {
        px.fill(Rgb565Pixel(0x0000));

        let t = (t_ms / 10) as i32;
        for i in 0..BARS {
            // 两个不同频率的正弦叠加，看起来比单一正弦更像频谱
            let a = sin_q10(t + i as i32 * 24) / 2;
            let b = sin_q10(t * 2 + i as i32 * 57) / 4;
            let target = ((a + b + 768) * MAX_H / 1536).clamp(0, MAX_H);

            self.level[i] += (target - self.level[i]) / 3;
            if self.level[i] > self.peak[i] {
                self.peak[i] = self.level[i];
            } else {
                self.peak[i] = (self.peak[i] - PEAK_DECAY).max(self.level[i]);
            }

            let color = level_color(self.level[i]);
            let left = i * BAR_W;
            let bottom = HEIGHT as i32 - 1;

            for h in 0..self.level[i] {
                let y = bottom - h;
                for x in left..left + BAR_W - 1 {
                    px[y as usize * WIDTH + x] = Rgb565Pixel(color);
                }
            }

            // 峰值处点两像素，用来读"最近的高点"
            let y = bottom - self.peak[i];
            if y >= 0 {
                for x in left..left + BAR_W - 1 {
                    px[y as usize * WIDTH + x] = Rgb565Pixel(0xffff);
                }
            }
        }
    }
}

/// 高度决定颜色：低绿、中黄、高红。
fn level_color(h: i32) -> u16 {
    if h * 3 < MAX_H {
        0x07e0
    } else if h * 3 < MAX_H * 2 {
        0xffe0
    } else {
        0xf800
    }
}
