//! 屏幕效果，一个效果一个文件。
//!
//! 每个效果都自己管重绘区域：先在内存里把一块像素算好，再一次性 `draw_raw_slice` 送出。
//! 中途不写屏，就不会出现「先擦后画」的黑场闪烁。

// `new` 只在 `main` 里调一次，不需要 Default 的另一种构造方式。
#![allow(clippy::new_without_default)]

pub mod balls;
pub mod cjk;
pub mod geometry;
pub mod plasma;
pub mod scroll;
pub mod wave;

/// 横屏后的可视宽度（像素）。
pub const DISPLAY_W: u16 = 320;
/// 横屏后的可视高度（像素）。
pub const DISPLAY_H: u16 = 240;

/// 角度（度）转 sin 定点值 Q10（1024 = 1.0）。Bhaskara 近似，误差 < 2%，够画动画。
pub fn sin_q10(deg: u16) -> i32 {
    let deg = deg % 360;
    let (x, sign) = if deg < 180 {
        (deg as i32, 1)
    } else {
        (deg as i32 - 180, -1)
    };
    let t = x * (180 - x);
    sign * (4 * t * 1024 / (40500 - t))
}
