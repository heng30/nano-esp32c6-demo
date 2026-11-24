//! 进度条：圆角边框 + 按比例的实心填充。

use crate::gfx::Frame;

/// 进度条：圆角边框 + 按比例的实心填充。
pub fn draw_progress_bar(
    frame: &mut Frame<'_>,
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
