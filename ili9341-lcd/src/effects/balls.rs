//! 多个不同颜色的小球碰壁反弹。

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
};

/// 小球数量。
const COUNT: usize = 8;
/// 小球半径（像素）。
const RADIUS: i32 = 12;
/// 位置/速度的定点比例：16 表示 1 像素。
const ONE: i32 = 16;
/// 帧数。
const FRAMES: u16 = 480;
/// 每帧等待时长。
const FRAME_DELAY: Duration = Duration::from_millis(10);
/// 重绘矩形缓冲边长 = 直径 + 两端各 1 像素余量 + 每帧最大位移 5 像素（Q4 的 80）。
/// `START` 里所有速度绝对值必须 ≤ 80，否则矩形会超出这个缓冲。
const BOX: usize = 2 * RADIUS as usize + 3 + 5;

/// 各球串行复用的重绘缓冲。
static BUF: StaticCell<[u16; BOX * BOX]> = StaticCell::new();

/// 各球的初始 (x, y, vx, vy)：位置是中心像素，速度是 Q4 定点。
const START: [(i32, i32, i32, i32); COUNT] = [
    (40, 40, 70, 50),
    (280, 60, -55, 75),
    (120, 200, 45, -80),
    (260, 190, -75, -55),
    (80, 120, 80, 60),
    (200, 50, -50, 70),
    (160, 140, 65, -65),
    (300, 120, -70, 45),
];
/// 各球的 RGB565 颜色：红、绿、蓝、黄、青、品红、橙、白。
const COLORS: [u16; COUNT] = [
    0xf800, 0x07e0, 0x001f, 0xffe0, 0x07ff, 0xf81f, 0xfd20, 0xffff,
];

/// 一个弹跳小球。位置与速度都取 Q4 定点，慢速时也不会因取整卡住。
#[derive(Clone, Copy)]
struct Ball {
    /// 中心 x（Q4）。
    x: i32,
    /// 中心 y（Q4）。
    y: i32,
    /// 上一帧的中心 x（Q4），用来算需要重画的矩形。
    prev_x: i32,
    /// 上一帧的中心 y（Q4）。
    prev_y: i32,
    /// 每帧 x 位移（Q4）。
    vx: i32,
    /// 每帧 y 位移（Q4）。
    vy: i32,
}

/// 屏幕里到处弹的小球。
pub struct Balls {
    /// 单个球重绘矩形的缓冲。
    buf: &'static mut [u16],
}

impl Balls {
    pub fn new() -> Self {
        Self {
            buf: BUF.init([0; BOX * BOX]),
        }
    }

    /// 每帧只重画每个球「旧位置 ∪ 新位置」的小矩形：整块在内存里算好再一次性送出，
    /// 没有先擦后画的中间态，也就不会闪。
    pub async fn run(&mut self, display: &mut Display) {
        info!("effect: bouncing balls");

        let bg = RawU16::from(Rgb565::BLACK).into_inner();
        let rq = RADIUS * ONE;
        let x_max = (DISPLAY_W as i32 - 1) * ONE - rq;
        let y_max = (DISPLAY_H as i32 - 1) * ONE - rq;
        let rr = RADIUS * RADIUS;

        let mut balls = [Ball {
            x: 0,
            y: 0,
            prev_x: 0,
            prev_y: 0,
            vx: 0,
            vy: 0,
        }; COUNT];
        for (ball, (x, y, vx, vy)) in balls.iter_mut().zip(START) {
            *ball = Ball {
                x: x * ONE,
                y: y * ONE,
                prev_x: x * ONE,
                prev_y: y * ONE,
                vx,
                vy,
            };
        }

        display.clear(Rgb565::BLACK).expect("clear failed");

        for _ in 0..FRAMES {
            for b in balls.iter_mut() {
                b.prev_x = b.x;
                b.prev_y = b.y;
                b.x += b.vx;
                b.y += b.vy;

                if b.x < rq {
                    b.x = rq;
                    b.vx = b.vx.abs();
                } else if b.x > x_max {
                    b.x = x_max;
                    b.vx = -b.vx.abs();
                }
                if b.y < rq {
                    b.y = rq;
                    b.vy = b.vy.abs();
                } else if b.y > y_max {
                    b.y = y_max;
                    b.vy = -b.vy.abs();
                }
            }

            for i in 0..COUNT {
                let b = &balls[i];
                let old_cx = b.prev_x / ONE;
                let old_cy = b.prev_y / ONE;
                let cx = b.x / ONE;
                let cy = b.y / ONE;

                let x0 = (old_cx.min(cx) - RADIUS - 1).max(0);
                let x1 = (old_cx.max(cx) + RADIUS + 1).min(DISPLAY_W as i32 - 1);
                let y0 = (old_cy.min(cy) - RADIUS - 1).max(0);
                let y1 = (old_cy.max(cy) + RADIUS + 1).min(DISPLAY_H as i32 - 1);
                let bw = (x1 - x0 + 1) as usize;

                for py in y0..=y1 {
                    for px in x0..=x1 {
                        let mut color = bg;
                        for (j, other) in balls.iter().enumerate() {
                            let dx = px - other.x / ONE;
                            let dy = py - other.y / ONE;
                            if dx * dx + dy * dy <= rr {
                                color = COLORS[j];
                                break;
                            }
                        }
                        self.buf[(py - y0) as usize * bw + (px - x0) as usize] = color;
                    }
                }

                let bh = (y1 - y0 + 1) as usize;
                display
                    .draw_raw_slice(
                        x0 as u16,
                        y0 as u16,
                        x1 as u16,
                        y1 as u16,
                        &self.buf[..bw * bh],
                    )
                    .expect("draw failed");
            }

            Timer::after(FRAME_DELAY).await;
        }
    }
}
