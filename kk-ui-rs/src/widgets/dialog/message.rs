//! 消息框弹窗：正文 + 单个确定键。

use crate::model::*;
use crate::runtime::{OverlayType, Runtime, Status};
use crate::widgets::utils;

/// 打开消息框。
pub(crate) fn open(rt: &mut Runtime, text: &'static str, event: EventId, now: u32) -> Status {
    super::begin(rt, OverlayType::Message, 0, text, event, now)
}

/// 处理输入：确定时入队事件并关闭。
pub(crate) fn input(rt: &mut Runtime, event: InputEvent, now: u32) {
    if event.action == InputAction::Ok && super::event_or_reject(rt, rt.overlay.event) {
        super::close(rt, now);
    }
}

/// 绘制标题、正文与确定键。
pub(crate) fn draw(rt: &mut Runtime<'_>, layout: &super::Layout) {
    let title = rt.app.texts.message_title;
    super::draw_title(rt, layout, title);
    let text = rt.overlay.text;
    utils::draw_centered(&mut rt.frame, 8 + layout.dx, 23 + layout.dy, 112, text);
    super::draw_buttons(rt, layout, false);
}
