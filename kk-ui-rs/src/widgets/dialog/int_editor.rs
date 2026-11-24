//! 整数编辑器弹窗：草稿值按步长增减，新旧值纵向滚动切换。

use crate::gfx::Frame;
use crate::model::{EVENT_NONE, InputAction, InputEvent, IntBinding};
use crate::runtime::{Overlay, OverlayType, Q12_ONE, Runtime, Status};

use crate::widgets::utils::{BUFFER_LEN, draw_centered, format_int};

/// 打开整数编辑器。
pub(crate) fn open(rt: &mut Runtime, index: u16, now: u32) -> Status {
    let Some(value) = rt
        .app
        .int_bindings
        .get(index as usize)
        .map(|binding| binding.value.get())
    else {
        return Status::InvalidArgument;
    };
    let status = super::begin(rt, OverlayType::Int, index, "", EVENT_NONE, now);
    if status == Status::Ok {
        rt.overlay.draft = value;
        rt.overlay.original = value;
        rt.overlay.previous = value;
    }
    status
}

/// 按步长调整整数草稿。
fn adjust(binding: &IntBinding, value: i32, increase: bool, steps: u16) -> i32 {
    let amount = binding.step as i64 * steps as i64;
    let next = value as i64 + if increase { amount } else { -amount };
    next.clamp(binding.minimum as i64, binding.maximum as i64) as i32
}

/// 改值：方向决定增减，结果夹在绑定的取值范围内。
pub(crate) fn change(rt: &mut Runtime, event: InputEvent, now: u32) {
    let increase = event.action == InputAction::Up;
    let draft = rt.overlay.draft;
    let binding = &rt.app.int_bindings[rt.overlay.reference as usize];
    let next = adjust(binding, draft, increase, event.steps);
    super::apply_value(rt, next, increase, now);
}

/// 提交编辑器结果。
pub(crate) fn commit(rt: &mut Runtime, now: u32) {
    let app = rt.app;
    let binding = &app.int_bindings[rt.overlay.reference as usize];
    let draft = rt.overlay.draft;
    if draft != rt.overlay.original {
        if !super::event_or_reject(rt, binding.changed_event) {
            return;
        }
        binding.value.set(draft);
    }
    super::close(rt, now);
}

/// 绘制标题、取值与按钮；编辑态额外画上下箭头。
pub(crate) fn draw(rt: &mut Runtime<'_>, layout: &super::Layout) {
    let app = rt.app;
    let overlay = rt.overlay;
    let binding = &app.int_bindings[overlay.reference as usize];
    super::draw_title(rt, layout, binding.title);
    draw_value(
        &overlay,
        binding.unit,
        &mut rt.frame,
        layout.dx,
        layout.dy,
    );
    super::draw_buttons(rt, layout, true);
    if rt.overlay.editing {
        let binding = &app.int_bindings[rt.overlay.reference as usize];
        let (dx, dy) = (layout.dx, layout.dy);
        if rt.overlay.draft < binding.maximum {
            rt.frame.draw_line(91 + dx, 26 + dy, 95 + dx, 22 + dy);
            rt.frame.draw_line(95 + dx, 22 + dy, 99 + dx, 26 + dy);
        }
        if rt.overlay.draft > binding.minimum {
            rt.frame.draw_line(91 + dx, 31 + dy, 95 + dx, 35 + dy);
            rt.frame.draw_line(95 + dx, 35 + dy, 99 + dx, 31 + dy);
        }
    }
}

/// 整数编辑器：新旧值纵向滚动切换。
fn draw_value(overlay: &Overlay, unit: &str, frame: &mut Frame<'_>, dx: i16, dy: i16) {
    let mut old_text = [0u8; BUFFER_LEN];
    let mut new_text = [0u8; BUFFER_LEN];
    let previous = format_int(overlay.previous, unit, &mut old_text);
    let draft = format_int(overlay.draft, unit, &mut new_text);
    frame.set_clip(39 + dx, 21 + dy, 46, 16);
    if overlay.value_moving {
        let offset = (16 * overlay.value_q12 as i32 / Q12_ONE as i32) as i16;
        if overlay.value_direction > 0 {
            draw_centered(frame, 39 + dx, 21 + dy - offset, 46, previous);
            draw_centered(frame, 39 + dx, 37 + dy - offset, 46, draft);
        } else {
            draw_centered(frame, 39 + dx, 21 + dy + offset, 46, previous);
            draw_centered(frame, 39 + dx, 5 + dy + offset, 46, draft);
        }
    } else {
        draw_centered(frame, 39 + dx, 21 + dy, 46, draft);
    }
}
