//! 确认框弹窗：正文 + 取消/确定，上下键在两个按钮间切换。

use crate::gfx::Frame;
use crate::model::*;
use crate::runtime::{DialogFocus, OverlayType, Runtime, Status};
use crate::widgets::utils;

/// 打开确认框。
pub(crate) fn open(rt: &mut Runtime, index: u16, now: u32) -> Status {
    let Some(text) = rt
        .app
        .confirm_descs
        .get(index as usize)
        .map(|desc| desc.text)
    else {
        return Status::InvalidArgument;
    };
    super::begin(rt, OverlayType::Confirm, index, text, EVENT_NONE, now)
}

/// 处理输入：确定执行当前焦点、上下键在两个按钮间切换。
pub(crate) fn input(rt: &mut Runtime, event: InputEvent, now: u32) {
    if event.action == InputAction::Ok {
        action(rt, now);
    } else {
        let focus = if rt.overlay.focus_mode == DialogFocus::Cancel {
            DialogFocus::Confirm
        } else {
            DialogFocus::Cancel
        };
        super::set_focus(rt, focus, now);
    }
}

/// 执行确认框当前焦点对应的动作。
fn action(rt: &mut Runtime, now: u32) {
    let desc = &rt.app.confirm_descs[rt.overlay.reference as usize];
    let event = if rt.overlay.focus_mode == DialogFocus::Confirm {
        desc.confirmed_event
    } else {
        desc.cancelled_event
    };
    if super::event_or_reject(rt, event) {
        super::close(rt, now);
    }
}

/// 绘制正文与按钮。
pub(crate) fn draw(rt: &mut Runtime<'_>, layout: &super::Layout) {
    let text = rt.overlay.text;
    draw_body(&mut rt.frame, layout.dx, layout.dy, text);
    super::draw_buttons(rt, layout, true);
}

/// 正文，按 `\n` 分两行居中。
fn draw_body(frame: &mut Frame<'_>, dx: i16, dy: i16, text: &str) {
    match text.split_once('\n') {
        Some((first, second)) => {
            utils::draw_centered(frame, 10 + dx, 4 + dy, 108, first);
            utils::draw_centered(frame, 10 + dx, 20 + dy, 108, second);
        }
        None => utils::draw_centered(frame, 10 + dx, 13 + dy, 108, text),
    }
}
