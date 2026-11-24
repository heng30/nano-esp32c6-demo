//! 焦点框：异或实心圆角矩形。

use crate::gfx::{DrawMode, Frame};

/// 画一个焦点框：异或实心圆角矩形。
pub fn draw_focus(frame: &mut Frame<'_>, x: i16, y: i16, width: u16, height: u16, radius: u16) {
    frame.set_draw_mode(DrawMode::Xor);
    frame.draw_rbox(x, y, width, height, radius);
    frame.set_draw_mode(DrawMode::Set);
}
