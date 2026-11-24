//! 各组件共用的绘图与格式化工具。

use crate::gfx::Frame;
use crate::model::PageRoute;
use crate::runtime::{PageState, Runtime};

/// 数字与单位后缀的缓冲容量。
pub(crate) const BUFFER_LEN: usize = 20;

/// 在 `[x, x+width)` 内居中绘制一行文字。
pub(crate) fn draw_centered(frame: &mut Frame<'_>, x: i16, y: i16, width: u16, text: &str) {
    let text_x = x as i32 + (width as i32 - frame.text_width(text) as i32) / 2;
    frame.draw_text(text_x as i16, y, text);
}

/// 把整数与单位后缀格式化进定长缓冲，返回其中的 UTF-8 片段。
pub fn format_int<'a>(value: i32, unit: &str, buffer: &'a mut [u8; BUFFER_LEN]) -> &'a str {
    let mut out = 0usize;
    if value < 0 {
        buffer[out] = b'-';
        out += 1;
    }
    let mut rest = value.unsigned_abs();
    let mut digits = [0u8; 10];
    let mut count = 0usize;
    loop {
        digits[count] = b'0' + (rest % 10) as u8;
        count += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    while count > 0 && out + 1 < buffer.len() {
        count -= 1;
        buffer[out] = digits[count];
        out += 1;
    }
    for byte in unit.as_bytes() {
        if out + 1 >= buffer.len() {
            break;
        }
        buffer[out] = *byte;
        out += 1;
    }
    core::str::from_utf8(&buffer[..out]).unwrap_or("")
}

/// 页面编号对应的路由；无页面或越界都返回 `None`。
pub(crate) fn route_page(rt: &Runtime, state: &PageState) -> Option<PageRoute> {
    rt.route(state.page)
}
