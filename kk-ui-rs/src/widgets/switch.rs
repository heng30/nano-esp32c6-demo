//! 开关：胶囊底 + 反色滑块。

use crate::gfx::{DrawMode, Frame};

/// 开关：胶囊底 + 反色滑块，`progress_q12` 控制滑动进度。
pub fn draw_switch(
    frame: &mut Frame<'_>,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    on: bool,
    progress_q12: u16,
) {
    if width < 7 || height < 7 || width < height {
        return;
    }
    let progress = progress_q12.min(4096);
    let radius = height / 2;
    let center_y = y + (height / 2) as i16;
    let left = x + (height / 2) as i16;
    let right = x + width as i16 - (height / 2) as i16;
    let (from, to) = if on { (left, right) } else { (right, left) };
    let knob_x = from + ((to - from) as i32 * progress as i32 / 4096) as i16;
    frame.draw_rbox(x, y, width, height, radius);
    frame.set_draw_mode(DrawMode::Clear);
    frame.draw_disc(knob_x, center_y, if radius > 2 { radius - 2 } else { 1 });
    frame.set_draw_mode(DrawMode::Set);
}
