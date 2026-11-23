//! 可复用图元：进度条与开关。

use crate::gfx::{DrawMode, Frame};

/// 画一个焦点框：异或实心圆角矩形。
pub fn draw_focus(frame: &mut Frame, x: i16, y: i16, width: u16, height: u16, radius: u16) {
    frame.set_draw_mode(DrawMode::Xor);
    frame.draw_rbox(x, y, width, height, radius);
    frame.set_draw_mode(DrawMode::Set);
}

/// 进度条：圆角边框 + 按比例的实心填充。
pub fn draw_progress_bar(
    frame: &mut Frame,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    value: u16,
    maximum: u16,
) {
    if width < 5 || height < 5 {
        return;
    }
    frame.draw_rframe(x, y, width, height, if height > 5 { 2 } else { 1 });
    if maximum == 0 || value == 0 {
        return;
    }
    let value = value.min(maximum);
    let inner_width = width - 4;
    let filled = ((inner_width as u32 * value as u32 + maximum as u32 / 2) / maximum as u32) as u16;
    if filled != 0 {
        frame.draw_rbox(
            x + 2,
            y + 2,
            filled,
            height - 4,
            if height > 7 { 2 } else { 1 },
        );
    }
}

/// 开关：胶囊底 + 反色滑块，`progress_q12` 控制滑动进度。
pub fn draw_switch(
    frame: &mut Frame,
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
