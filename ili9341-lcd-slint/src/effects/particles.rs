//! 粒子系统：重力、边界反弹、整屏衰减拖尾。

use slint::platform::software_renderer::Rgb565Pixel;

use super::{Canvas, HEIGHT, WIDTH};

/// 粒子数量。
const COUNT: usize = 32;
/// 定点比例：256 表示 1 像素。
const ONE: i32 = 256;
/// 每帧重力加速度（定点）。
const GRAVITY: i32 = 5;
/// 反弹后保留的速度比例（Q8，200/256 ≈ 78%）。
const BOUNCE: i32 = 200;

/// 一个粒子。位置与速度都取 Q8 定点，慢速时不会因取整停住。
#[derive(Clone, Copy)]
struct Particle {
    /// x（Q8）。
    x: i32,
    /// y（Q8）。
    y: i32,
    /// 每帧 x 位移（Q8）。
    vx: i32,
    /// 每帧 y 位移（Q8）。
    vy: i32,
}

/// 粒子颜色：青、黄、品红、白轮转。
const COLORS: [u16; 4] = [0x07ff, 0xffe0, 0xf81f, 0xffff];

/// 满屏乱撞的粒子，靠整屏变暗留下拖尾。
pub struct Particles {
    /// 粒子状态。
    items: [Particle; COUNT],
}

impl Particles {
    pub fn new() -> Self {
        // 线性同余发生器，只为每个粒子取到不同的初值
        let mut seed = 0x1234_5678u32;
        let mut items = [Particle {
            x: 0,
            y: 0,
            vx: 0,
            vy: 0,
        }; COUNT];

        for item in items.iter_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = ((seed >> 8) % WIDTH as u32) as i32 * ONE;
            let y = ((seed >> 20) % HEIGHT as u32) as i32 * ONE;
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let vx = ((seed >> 8) % 21) as i32 - 10;
            let vy = -(((seed >> 16) % 16) as i32) - 4;

            *item = Particle {
                x,
                y,
                vx: vx * ONE / 16,
                vy: vy * ONE / 16,
            };
        }

        Self { items }
    }
}

impl Canvas for Particles {
    fn draw(&mut self, px: &mut [Rgb565Pixel], _t_ms: u32) {
        // 整屏变暗一半，粒子走过的地方留下渐隐的尾巴
        for p in px.iter_mut() {
            p.0 = dim(p.0);
        }

        let max_x = (WIDTH as i32 - 1) * ONE;
        let max_y = (HEIGHT as i32 - 1) * ONE;

        for (i, item) in self.items.iter_mut().enumerate() {
            item.vy += GRAVITY;
            item.x += item.vx;
            item.y += item.vy;

            if item.x < 0 {
                item.x = 0;
                item.vx = -item.vx * BOUNCE / ONE;
            } else if item.x > max_x {
                item.x = max_x;
                item.vx = -item.vx * BOUNCE / ONE;
            }
            if item.y > max_y {
                item.y = max_y;
                item.vy = -item.vy * BOUNCE / ONE;
            } else if item.y < 0 {
                item.y = 0;
                item.vy = -item.vy * BOUNCE / ONE;
            }

            let color = COLORS[i % COLORS.len()];
            let (x, y) = (item.x / ONE, item.y / ONE);
            for dy in 0..2 {
                for dx in 0..2 {
                    let (px_x, px_y) = (x + dx, y + dy);
                    if px_x >= 0 && px_x < WIDTH as i32 && px_y >= 0 && px_y < HEIGHT as i32 {
                        px[px_y as usize * WIDTH + px_x as usize] = Rgb565Pixel(color);
                    }
                }
            }
        }
    }
}

/// RGB565 每个通道右移一位，颜色减半。
fn dim(color: u16) -> u16 {
    let r = (color >> 11) & 0x1f;
    let g = (color >> 5) & 0x3f;
    let b = color & 0x1f;
    ((r >> 1) << 11) | ((g >> 1) << 5) | (b >> 1)
}
