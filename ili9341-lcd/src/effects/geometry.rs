//! 旋转的三角形 + 中心固定的圆。

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

/// 三角形的外接圆半径（像素）。
const RADIUS: i32 = 90;
/// 重绘区域边长：既是三角形的外接方形，也是局部帧缓冲的边长。
const BOX: u16 = 2 * RADIUS as u16;
/// 中心固定圆的半径（像素）。
const DOT: i32 = 25;
/// 每帧旋转的角度（度）。
const STEP: u16 = 2;
/// 帧数：正好转满一圈。
const FRAMES: u16 = 360 / STEP;
/// 每帧等待时长。
const FRAME_DELAY: Duration = Duration::from_millis(10);

/// 局部帧缓冲，整块算好再一次送出；`StaticCell` 只能初始化一次，所以由结构体持有。
static FRAME: StaticCell<[u16; BOX as usize * BOX as usize]> = StaticCell::new();

/// 绕屏心旋转的三角形，圆压在三角形上面。
pub struct Geometry {
    /// 外接方形区域的重绘缓冲。
    frame: &'static mut [u16],
}

impl Geometry {
    pub fn new() -> Self {
        Self {
            frame: FRAME.init([0; BOX as usize * BOX as usize]),
        }
    }

    pub async fn run(&mut self, display: &mut Display) {
        info!("effect: geometry");

        let half = (BOX / 2) as i32;
        let left = DISPLAY_W / 2 - BOX / 2;
        let top = DISPLAY_H / 2 - BOX / 2;
        let dot2 = DOT * DOT;
        let tri_color = RawU16::from(Rgb565::CYAN).into_inner();
        let dot_color = RawU16::from(Rgb565::MAGENTA).into_inner();
        let bg_color = RawU16::from(Rgb565::BLACK).into_inner();

        display.clear(Rgb565::BLACK).expect("clear failed");

        for frame in 0..FRAMES {
            let pts = vertices(frame * STEP);
            for (row, line) in self.frame.chunks_mut(BOX as usize).enumerate() {
                let y = row as i32 - half;
                for (col, px) in line.iter_mut().enumerate() {
                    let p = Point::new(col as i32 - half, y);
                    // 圆画在三角形上面，否则三角内部的实心区域会把它整个盖掉
                    *px = if p.x * p.x + y * y <= dot2 {
                        dot_color
                    } else if inside(&pts, p) {
                        tri_color
                    } else {
                        bg_color
                    };
                }
            }

            display
                .draw_raw_slice(left, top, left + BOX - 1, top + BOX - 1, self.frame)
                .expect("draw failed");
            Timer::after(FRAME_DELAY).await;
        }
    }
}

/// 以方框中心为原点的三角形顶点。
fn vertices(angle: u16) -> [Point; 3] {
    let mut pts = [Point::new(0, 0); 3];
    for (i, p) in pts.iter_mut().enumerate() {
        let deg = angle + i as u16 * 120;
        *p = Point::new(
            sin_q10(deg) * RADIUS / 1024,
            -sin_q10(deg + 90) * RADIUS / 1024,
        );
    }
    pts
}

/// 向量 ab 与 ap 的叉积，用来判断点落在某条边的哪一侧。
fn cross(a: Point, b: Point, p: Point) -> i32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

fn inside(pts: &[Point; 3], p: Point) -> bool {
    let a = cross(pts[0], pts[1], p);
    let b = cross(pts[1], pts[2], p);
    let c = cross(pts[2], pts[0], p);
    (a >= 0 && b >= 0 && c >= 0) || (a <= 0 && b <= 0 && c <= 0)
}
