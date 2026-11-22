//! 旋转的三角形 + 中心固定的圆，全屏逐像素画。

use slint::platform::software_renderer::Rgb565Pixel;

use super::{Canvas, HEIGHT, WIDTH, sin_q10};

/// 三角形的外接圆半径（像素）。
const RADIUS: i32 = 45;
/// 中心固定圆的半径（像素）。
const DOT: i32 = 12;
/// 线程色（RGB565 青）。
const TRI_COLOR: u16 = 0x07ff;
/// 圆心色（RGB565 品红）。
const DOT_COLOR: u16 = 0xf81f;
/// 背景色（RGB565 黑）。
const BG: u16 = 0x0000;

/// 绕屏心旋转的三角形，圆压在三角形上面。
pub struct Geometry;

impl Canvas for Geometry {
    fn draw(&mut self, px: &mut [Rgb565Pixel], t_ms: u32) {
        let pts = vertices((t_ms / 8) as i32);
        let cx = (WIDTH / 2) as i32;
        let cy = (HEIGHT / 2) as i32;
        let dot2 = DOT * DOT;

        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let p = (x as i32 - cx, y as i32 - cy);
                let color = if p.0 * p.0 + p.1 * p.1 <= dot2 {
                    DOT_COLOR
                } else if inside(&pts, p) {
                    TRI_COLOR
                } else {
                    BG
                };
                px[y * WIDTH + x] = Rgb565Pixel(color);
            }
        }
    }
}

/// 以屏心为原点的三角形顶点。
fn vertices(angle: i32) -> [(i32, i32); 3] {
    let mut pts = [(0, 0); 3];
    for (i, p) in pts.iter_mut().enumerate() {
        let deg = angle + i as i32 * 120;
        *p = (
            sin_q10(deg) * RADIUS / 1024,
            -sin_q10(deg + 90) * RADIUS / 1024,
        );
    }
    pts
}

/// 向量 ab 与 ap 的叉积，用来判断点落在某条边的哪一侧。
fn cross(a: (i32, i32), b: (i32, i32), p: (i32, i32)) -> i32 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

fn inside(pts: &[(i32, i32); 3], p: (i32, i32)) -> bool {
    let a = cross(pts[0], pts[1], p);
    let b = cross(pts[1], pts[2], p);
    let c = cross(pts[2], pts[0], p);
    (a >= 0 && b >= 0 && c >= 0) || (a <= 0 && b <= 0 && c <= 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_is_inside_triangle() {
        for angle in 0..360 {
            assert!(inside(&vertices(angle), (0, 0)), "angle {angle}");
        }
    }

    #[test]
    fn far_point_is_outside_triangle() {
        for angle in 0..360 {
            assert!(!inside(&vertices(angle), (RADIUS * 2, 0)), "angle {angle}");
        }
    }
}
