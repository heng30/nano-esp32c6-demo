//! 开关编辑器弹窗：关/开文字与滑动滑块。

use crate::gfx::Frame;
use crate::model::*;
use crate::runtime::{Overlay, OverlayType, Q12_ONE, Runtime, Status};
use crate::widgets::utils;

/// 打开开关编辑器。
pub(crate) fn open(rt: &mut Runtime, index: u16, now: u32) -> Status {
    let Some(value) = rt
        .app
        .bool_bindings
        .get(index as usize)
        .map(|binding| i32::from(binding.value.get()))
    else {
        return Status::InvalidArgument;
    };
    let status = super::begin(rt, OverlayType::Bool, index, "", EVENT_NONE, now);
    if status == Status::Ok {
        rt.overlay.draft = value;
        rt.overlay.original = value;
        rt.overlay.previous = value;
    }
    status
}

/// 改值：开关没有方向语义，任意方向都翻转草稿。
pub(crate) fn change(rt: &mut Runtime, event: InputEvent, now: u32) {
    // 长按重复与偶数步不处理，避免自激振荡。
    if event.source == InputSource::Repeat || event.steps & 1 == 0 {
        return;
    }
    let next = i32::from(rt.overlay.draft == 0);
    let increase = next != 0;
    super::apply_value(rt, next, increase, now);
}

/// 提交编辑器结果。
pub(crate) fn commit(rt: &mut Runtime, now: u32) {
    let app = rt.app;
    let binding = &app.bool_bindings[rt.overlay.reference as usize];
    let draft = rt.overlay.draft;
    if draft != rt.overlay.original {
        if !super::event_or_reject(rt, binding.changed_event) {
            return;
        }
        binding.value.set(draft != 0);
    }
    super::close(rt, now);
}

/// 绘制标题、开关与按钮。
pub(crate) fn draw(rt: &mut Runtime<'_>, layout: &super::Layout) {
    let app = rt.app;
    let overlay = rt.overlay;
    let binding = &app.bool_bindings[overlay.reference as usize];
    super::draw_title(rt, layout, binding.title);
    draw_value(
        &overlay,
        app.texts.on_text,
        app.texts.off_text,
        &mut rt.frame,
        layout.dx,
        layout.dy,
    );
    super::draw_buttons(rt, layout, true);
}

/// 开关编辑器：关/开文字与滑块。
fn draw_value(
    overlay: &Overlay,
    on_text: &str,
    off_text: &str,
    frame: &mut Frame<'_>,
    dx: i16,
    dy: i16,
) {
    let on = overlay.draft != 0;
    let progress = if overlay.value_moving {
        overlay.value_q12
    } else {
        Q12_ONE
    };
    utils::draw_centered(frame, 31 + dx, 21 + dy, 14, off_text);
    utils::draw_centered(frame, 83 + dx, 21 + dy, 14, on_text);
    crate::widgets::draw_switch(frame, 48 + dx, 22 + dy, 33, 14, on, progress);
}
