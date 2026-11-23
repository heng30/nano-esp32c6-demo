//! 弹窗覆盖层与 Toast：整数编辑器、开关编辑器、确认框、消息框、提示条。

use crate::gfx::{DrawMode, Frame, HEIGHT, WIDTH};
use crate::model::*;
use crate::runtime::{
    BOOL_VALUE_MS, DIALOG_CLOSE_MS, DIALOG_OPEN_MS, DialogFocus, FOCUS_MS, INT_VALUE_MS,
    MENU_HEADER_HEIGHT, Overlay, OverlayType, Q12_ONE, Runtime, Status, TOAST_MS, ease_q12,
    lerp_q12, round_q8,
};

/// 文本缓冲容量。
const BUFFER_LEN: usize = 20;

/// 在 `[x, x+width)` 内居中绘制一行文字。
pub(crate) fn draw_centered(frame: &mut Frame, x: i16, y: i16, width: u16, text: &str) {
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

/// 是否有覆盖层占用输入。
pub(crate) fn dialog_active(rt: &Runtime) -> bool {
    rt.overlay.kind != OverlayType::None
}

/// 弹窗出现时对齐的行号：菜单页对齐当前焦点行，其它页固定在 24。
fn dialog_origin(rt: &Runtime) -> i16 {
    if rt.current_page_type() == PageType::Menu {
        MENU_HEADER_HEIGHT + round_q8(rt.current.focus_q8)
    } else {
        24
    }
}

/// 焦点框的目标矩形。
fn focus_rect(rt: &Runtime, focus: DialogFocus) -> (i16, i16, i16, i16) {
    if rt.overlay.kind == OverlayType::Message {
        (46, 41, 36, 16)
    } else if focus == DialogFocus::Value {
        let bool_editor = rt.overlay.kind == OverlayType::Bool;
        (
            if bool_editor { 29 } else { 39 },
            21,
            if bool_editor { 70 } else { 46 },
            16,
        )
    } else if focus == DialogFocus::Confirm {
        (77, 41, 36, 16)
    } else {
        (15, 41, 36, 16)
    }
}

/// 当前（可能仍在动画中的）焦点矩形，Q8 定点。
fn current_focus(rt: &Runtime, now: u32) -> (i32, i32, i32, i32) {
    if rt.overlay.focus_moving {
        let p = ease_q12(now.wrapping_sub(rt.overlay.focus_started), FOCUS_MS);
        (
            lerp_q12(rt.overlay.focus_from.0, rt.overlay.focus.0, p),
            lerp_q12(rt.overlay.focus_from.1, rt.overlay.focus.1, p),
            lerp_q12(rt.overlay.focus_from.2, rt.overlay.focus.2, p),
            lerp_q12(rt.overlay.focus_from.3, rt.overlay.focus.3, p),
        )
    } else {
        rt.overlay.focus
    }
}

/// 切换焦点并启动焦点框补间。
fn set_focus(rt: &mut Runtime, focus: DialogFocus, now: u32) {
    let from = current_focus(rt, now);
    let (x, y, width, height) = focus_rect(rt, focus);
    rt.overlay.focus_from = from;
    rt.overlay.focus = (
        x as i32 * 256,
        y as i32 * 256,
        width as i32 * 256,
        height as i32 * 256,
    );
    rt.overlay.focus_started = now;
    rt.overlay.focus_moving = true;
    rt.dirty = true;
}

/// 打开一个覆盖层。
fn begin(
    rt: &mut Runtime,
    kind: OverlayType,
    reference: u16,
    text: &'static str,
    event: EventId,
    now: u32,
) -> Status {
    if dialog_active(rt) {
        return Status::Busy;
    }
    rt.toast.visible = false;
    rt.overlay.kind = kind;
    rt.overlay.reference = reference;
    rt.overlay.text = text;
    rt.overlay.event = event;
    rt.overlay.origin_y = dialog_origin(rt);
    rt.overlay.phase_started = now;
    rt.overlay.phase_q12 = 0;
    rt.overlay.phase_from_q12 = 0;
    rt.overlay.value_q12 = Q12_ONE;
    rt.overlay.closing = false;
    rt.overlay.focus_moving = false;
    rt.overlay.value_moving = false;
    let focus = match kind {
        OverlayType::Confirm => DialogFocus::Cancel,
        OverlayType::Message => DialogFocus::Confirm,
        _ => DialogFocus::Value,
    };
    rt.overlay.focus_mode = focus;
    rt.overlay.editing = matches!(kind, OverlayType::Int | OverlayType::Bool);
    let (x, y, width, height) = focus_rect(rt, focus);
    rt.overlay.focus = (
        x as i32 * 256,
        y as i32 * 256,
        width as i32 * 256,
        height as i32 * 256,
    );
    rt.overlay.focus_from = rt.overlay.focus;
    rt.dirty = true;
    Status::Ok
}

/// 打开整数编辑器。
pub(crate) fn open_int(rt: &mut Runtime, index: u16, now: u32) -> Status {
    let Some(value) = rt
        .app
        .int_bindings
        .get(index as usize)
        .map(|binding| binding.value.get())
    else {
        return Status::InvalidArgument;
    };
    let status = begin(rt, OverlayType::Int, index, "", EVENT_NONE, now);
    if status == Status::Ok {
        rt.overlay.draft = value;
        rt.overlay.original = value;
        rt.overlay.previous = value;
    }
    status
}

/// 打开开关编辑器。
pub(crate) fn open_bool(rt: &mut Runtime, index: u16, now: u32) -> Status {
    let Some(value) = rt
        .app
        .bool_bindings
        .get(index as usize)
        .map(|binding| i32::from(binding.value.get()))
    else {
        return Status::InvalidArgument;
    };
    let status = begin(rt, OverlayType::Bool, index, "", EVENT_NONE, now);
    if status == Status::Ok {
        rt.overlay.draft = value;
        rt.overlay.original = value;
        rt.overlay.previous = value;
    }
    status
}

/// 打开确认框。
pub(crate) fn open_confirm(rt: &mut Runtime, index: u16, now: u32) -> Status {
    let Some(text) = rt
        .app
        .confirm_descs
        .get(index as usize)
        .map(|desc| desc.text)
    else {
        return Status::InvalidArgument;
    };
    begin(rt, OverlayType::Confirm, index, text, EVENT_NONE, now)
}

/// 打开消息框。
pub(crate) fn open_message(
    rt: &mut Runtime,
    text: &'static str,
    event: EventId,
    now: u32,
) -> Status {
    begin(rt, OverlayType::Message, 0, text, event, now)
}

/// 打开 Toast；`duration` 为 0 时用默认时长。
pub(crate) fn toast_open(rt: &mut Runtime, text: &'static str, duration: u32, now: u32) -> Status {
    if dialog_active(rt) {
        return Status::Busy;
    }
    let duration = if duration == 0 {
        1500
    } else {
        duration.min(i32::MAX as u32)
    };
    rt.toast.text = text;
    rt.toast.shown_at = now;
    rt.toast.expires_at = now.wrapping_add(duration);
    rt.toast.phase_q12 = 0;
    rt.toast.visible = true;
    rt.toast.closing = false;
    rt.dirty = true;
    Status::Ok
}

/// 开始关闭动画。
fn close(rt: &mut Runtime, now: u32) {
    rt.overlay.phase_from_q12 = rt.overlay.phase_q12;
    rt.overlay.phase_started = now;
    rt.overlay.closing = true;
    rt.overlay.editing = false;
    rt.dirty = true;
}

/// 入队事件；队列满时记录错误并闪一下边框。
fn event_or_reject(rt: &mut Runtime, event: EventId) -> bool {
    if event == EVENT_NONE || rt.queue_event(event) {
        return true;
    }
    rt.record_error(Status::QueueFull, rt.current.page, event);
    rt.reject_flash = 2;
    rt.dirty = true;
    false
}

/// 按步长调整整数草稿。
fn int_adjust(binding: &IntBinding, value: i32, increase: bool, steps: u16) -> i32 {
    let amount = binding.step as i64 * steps as i64;
    let next = value as i64 + if increase { amount } else { -amount };
    next.clamp(binding.minimum as i64, binding.maximum as i64) as i32
}

/// 修改编辑器当前值。
fn change_value(rt: &mut Runtime, event: InputEvent, now: u32) {
    let mut next = rt.overlay.draft;
    let mut increase = event.action == InputAction::Up;
    match rt.overlay.kind {
        OverlayType::Int => {
            let binding = &rt.app.int_bindings[rt.overlay.reference as usize];
            next = int_adjust(binding, next, increase, event.steps);
        }
        OverlayType::Bool => {
            // 开关没有方向语义：任意方向都翻转；长按重复与偶数步不处理，避免自激振荡。
            if event.source == InputSource::Repeat || event.steps & 1 == 0 {
                return;
            }
            next = i32::from(rt.overlay.draft == 0);
            increase = next != 0;
        }
        _ => return,
    }
    if next != rt.overlay.draft {
        rt.overlay.previous = rt.overlay.draft;
        rt.overlay.draft = next;
        rt.overlay.value_direction = if increase { 1 } else { -1 };
        rt.overlay.value_q12 = 0;
        rt.overlay.value_started = now;
        rt.overlay.value_moving = true;
        rt.dirty = true;
    }
}

/// 提交编辑器结果。
fn commit(rt: &mut Runtime, now: u32) {
    let app = rt.app;
    let reference = rt.overlay.reference as usize;
    let draft = rt.overlay.draft;
    let changed = draft != rt.overlay.original;
    match rt.overlay.kind {
        OverlayType::Int => {
            let binding = &app.int_bindings[reference];
            if changed {
                if !event_or_reject(rt, binding.changed_event) {
                    return;
                }
                binding.value.set(draft);
            }
        }
        OverlayType::Bool => {
            let binding = &app.bool_bindings[reference];
            if changed {
                if !event_or_reject(rt, binding.changed_event) {
                    return;
                }
                binding.value.set(draft != 0);
            }
        }
        _ => {}
    }
    close(rt, now);
}

/// 执行确认框当前焦点对应的动作。
fn confirm_action(rt: &mut Runtime, now: u32) {
    let desc = &rt.app.confirm_descs[rt.overlay.reference as usize];
    let event = if rt.overlay.focus_mode == DialogFocus::Confirm {
        desc.confirmed_event
    } else {
        desc.cancelled_event
    };
    if event_or_reject(rt, event) {
        close(rt, now);
    }
}

/// 处理输入。
pub(crate) fn input(rt: &mut Runtime, event: InputEvent, now: u32) {
    if !dialog_active(rt) || rt.overlay.closing {
        return;
    }
    match rt.overlay.kind {
        OverlayType::Message => {
            if event.action == InputAction::Ok && event_or_reject(rt, rt.overlay.event) {
                close(rt, now);
            }
        }
        OverlayType::Confirm => {
            if event.action == InputAction::Ok {
                confirm_action(rt, now);
            } else {
                let focus = if rt.overlay.focus_mode == DialogFocus::Cancel {
                    DialogFocus::Confirm
                } else {
                    DialogFocus::Cancel
                };
                set_focus(rt, focus, now);
            }
        }
        OverlayType::Int | OverlayType::Bool => {
            if rt.overlay.editing {
                if event.action == InputAction::Ok {
                    rt.overlay.editing = false;
                    set_focus(rt, DialogFocus::Confirm, now);
                } else {
                    change_value(rt, event, now);
                }
                return;
            }
            if event.action == InputAction::Ok {
                match rt.overlay.focus_mode {
                    DialogFocus::Value => {
                        rt.overlay.editing = true;
                        rt.dirty = true;
                    }
                    DialogFocus::Confirm => commit(rt, now),
                    DialogFocus::Cancel => close(rt, now),
                }
                return;
            }
            let next = match rt.overlay.focus_mode {
                DialogFocus::Value => {
                    if event.action == InputAction::Up {
                        DialogFocus::Cancel
                    } else {
                        DialogFocus::Confirm
                    }
                }
                DialogFocus::Confirm => {
                    if event.action == InputAction::Up {
                        DialogFocus::Value
                    } else {
                        DialogFocus::Cancel
                    }
                }
                DialogFocus::Cancel => {
                    if event.action == InputAction::Up {
                        DialogFocus::Confirm
                    } else {
                        DialogFocus::Value
                    }
                }
            };
            set_focus(rt, next, now);
        }
        OverlayType::None => {}
    }
}

/// Toast 动画：先滑入，到期后滑出。
fn toast_animate(rt: &mut Runtime, now: u32) {
    if !rt.toast.visible {
        return;
    }
    if !rt.toast.closing && (now.wrapping_sub(rt.toast.expires_at) as i32) >= 0 {
        rt.toast.closing = true;
        rt.toast.shown_at = now;
    }
    if rt.toast.closing {
        let p = ease_q12(now.wrapping_sub(rt.toast.shown_at), TOAST_MS);
        rt.toast.phase_q12 = Q12_ONE - p;
        rt.dirty = true;
        if p == Q12_ONE {
            rt.toast.visible = false;
        }
    } else if rt.toast.phase_q12 < Q12_ONE {
        rt.toast.phase_q12 = ease_q12(now.wrapping_sub(rt.toast.shown_at), TOAST_MS);
        rt.dirty = true;
    }
}

/// 覆盖层与 Toast 的逐帧动画。
pub(crate) fn animate(rt: &mut Runtime, now: u32) {
    if dialog_active(rt) {
        let elapsed = now.wrapping_sub(rt.overlay.phase_started);
        let mut redraw = false;
        if rt.overlay.closing {
            let p = ease_q12(elapsed, DIALOG_CLOSE_MS);
            rt.overlay.phase_q12 = lerp_q12(rt.overlay.phase_from_q12 as i32, 0, p) as u16;
            redraw = true;
            if p == Q12_ONE {
                rt.overlay.kind = OverlayType::None;
                rt.overlay.closing = false;
            }
        } else if rt.overlay.phase_q12 < Q12_ONE {
            rt.overlay.phase_q12 = ease_q12(elapsed, DIALOG_OPEN_MS);
            redraw = true;
        }
        if rt.overlay.focus_moving {
            redraw = true;
            if now.wrapping_sub(rt.overlay.focus_started) >= FOCUS_MS {
                rt.overlay.focus_moving = false;
            }
        }
        if rt.overlay.value_moving {
            let duration = if rt.overlay.kind == OverlayType::Bool {
                BOOL_VALUE_MS
            } else {
                INT_VALUE_MS
            };
            redraw = true;
            rt.overlay.value_q12 = ease_q12(now.wrapping_sub(rt.overlay.value_started), duration);
            if rt.overlay.value_q12 == Q12_ONE {
                rt.overlay.value_moving = false;
            }
        }
        if redraw {
            rt.dirty = true;
        }
    }
    toast_animate(rt, now);
}

/// 覆盖层标题。
fn dialog_title(rt: &Runtime) -> &'static str {
    let app = rt.app;
    match rt.overlay.kind {
        OverlayType::Int => app.int_bindings[rt.overlay.reference as usize].title,
        OverlayType::Bool => app.bool_bindings[rt.overlay.reference as usize].title,
        _ => app.texts.message_title,
    }
}

/// 确认框正文，按 `\n` 分两行居中。
fn draw_dialog_text(frame: &mut Frame, text: &str) {
    match text.split_once('\n') {
        Some((first, second)) => {
            draw_centered(frame, 10, 4, 108, first);
            draw_centered(frame, 10, 20, 108, second);
        }
        None => draw_centered(frame, 10, 13, 108, text),
    }
}

/// 整数编辑器：新旧值纵向滚动切换。
fn draw_int_value(overlay: &Overlay, unit: &str, frame: &mut Frame) {
    let mut old_text = [0u8; BUFFER_LEN];
    let mut new_text = [0u8; BUFFER_LEN];
    let previous = format_int(overlay.previous, unit, &mut old_text);
    let draft = format_int(overlay.draft, unit, &mut new_text);
    frame.set_clip(39, 21, 46, 16);
    if overlay.value_moving {
        let offset = (16 * overlay.value_q12 as i32 / Q12_ONE as i32) as i16;
        if overlay.value_direction > 0 {
            draw_centered(frame, 39, 21 - offset, 46, previous);
            draw_centered(frame, 39, 37 - offset, 46, draft);
        } else {
            draw_centered(frame, 39, 21 + offset, 46, previous);
            draw_centered(frame, 39, 5 + offset, 46, draft);
        }
    } else {
        draw_centered(frame, 39, 21, 46, draft);
    }
}

/// 开关编辑器：关/开文字与滑块。
fn draw_bool_value(overlay: &Overlay, on_text: &str, off_text: &str, frame: &mut Frame) {
    let on = overlay.draft != 0;
    let progress = if overlay.value_moving {
        overlay.value_q12
    } else {
        Q12_ONE
    };
    draw_centered(frame, 31, 21, 14, off_text);
    draw_centered(frame, 83, 21, 14, on_text);
    crate::helpers::draw_switch(frame, 48, 22, 33, 14, on, progress);
}

/// 绘制覆盖层。
pub(crate) fn draw(rt: &mut Runtime) {
    if !dialog_active(rt) {
        return;
    }
    let p = rt.overlay.phase_q12 as i32;
    let one = Q12_ONE as i32;
    let x = (3 + 3 * p / one) as i16;
    let y = (rt.overlay.origin_y as i32 + (1 - rt.overlay.origin_y as i32) * p / one) as i16;
    let width = (118 - 2 * p / one) as i16;
    let height = (16 + 45 * p / one) as i16;
    {
        let frame = &mut rt.frame;
        frame.reset_clip();
        frame.set_draw_mode(DrawMode::Clear);
        frame.draw_rbox(x, y, width as u16, height as u16, 3);
        frame.set_draw_mode(DrawMode::Set);
        frame.draw_rframe(x, y, width as u16, height as u16, 3);
        if width <= 2 || height <= 2 {
            return;
        }
        frame.set_clip(x + 1, y + 1, (width - 2) as u16, (height - 2) as u16);
        frame.set_font(rt.app.fonts.body);
    }
    match rt.overlay.kind {
        OverlayType::Confirm => {
            let text = rt.overlay.text;
            draw_dialog_text(&mut rt.frame, text);
            draw_centered(&mut rt.frame, 15, 41, 36, rt.app.texts.cancel_text);
            draw_centered(&mut rt.frame, 77, 41, 36, rt.app.texts.confirm_text);
        }
        kind => {
            let title = dialog_title(rt);
            draw_centered(&mut rt.frame, 10, 3, 108, title);
            rt.frame.draw_hline(12, 19, 104);
            match kind {
                OverlayType::Int => {
                    let overlay = rt.overlay;
                    let app = rt.app;
                    let unit = app.int_bindings[overlay.reference as usize].unit;
                    draw_int_value(&overlay, unit, &mut rt.frame);
                    rt.frame
                        .set_clip(x + 1, y + 1, (width - 2) as u16, (height - 2) as u16);
                    draw_centered(&mut rt.frame, 15, 41, 36, rt.app.texts.cancel_text);
                    draw_centered(&mut rt.frame, 77, 41, 36, rt.app.texts.confirm_text);
                    if rt.overlay.editing {
                        let binding = &rt.app.int_bindings[rt.overlay.reference as usize];
                        if rt.overlay.draft < binding.maximum {
                            rt.frame.draw_line(91, 26, 95, 22);
                            rt.frame.draw_line(95, 22, 99, 26);
                        }
                        if rt.overlay.draft > binding.minimum {
                            rt.frame.draw_line(91, 31, 95, 35);
                            rt.frame.draw_line(95, 35, 99, 31);
                        }
                    }
                }
                OverlayType::Bool => {
                    let overlay = rt.overlay;
                    let app = rt.app;
                    draw_bool_value(
                        &overlay,
                        app.texts.on_text,
                        app.texts.off_text,
                        &mut rt.frame,
                    );
                    draw_centered(&mut rt.frame, 15, 41, 36, rt.app.texts.cancel_text);
                    draw_centered(&mut rt.frame, 77, 41, 36, rt.app.texts.confirm_text);
                }
                _ => {
                    let text = rt.overlay.text;
                    draw_centered(&mut rt.frame, 8, 23, 112, text);
                    draw_centered(&mut rt.frame, 46, 41, 36, rt.app.texts.confirm_text);
                }
            }
        }
    }
    let (focus_x, focus_y, focus_w, focus_h) = current_focus(rt, rt.last_update);
    rt.frame.set_draw_mode(DrawMode::Xor);
    rt.frame.draw_rbox(
        round_q8(focus_x),
        round_q8(focus_y),
        round_q8(focus_w) as u16,
        round_q8(focus_h) as u16,
        3,
    );
    rt.frame.set_draw_mode(DrawMode::Set);
    rt.frame.reset_clip();
}

/// 绘制 Toast。
pub(crate) fn draw_toast(rt: &mut Runtime) {
    if !rt.toast.visible || dialog_active(rt) {
        return;
    }
    rt.frame.set_font(rt.app.fonts.body);
    let mut width = rt.frame.text_width(rt.toast.text) + 12;
    if width > 120 {
        width = 120;
    }
    if width & 1 != 0 {
        width += 1;
    }
    let x = (WIDTH - width as i16) / 2;
    let y = HEIGHT - (16 * rt.toast.phase_q12 as i32 / Q12_ONE as i32) as i16;
    rt.frame.set_clip(x, y, width, 16);
    // 先清空整个包围盒，圆角外的像素才不会带着底下的页面字形。
    rt.frame.set_draw_mode(DrawMode::Clear);
    rt.frame.draw_box(x, y, width, 16);
    rt.frame.set_draw_mode(DrawMode::Set);
    rt.frame.draw_rbox(x, y, width, 16, 3);
    rt.frame.set_draw_mode(DrawMode::Clear);
    draw_centered(&mut rt.frame, x, y + 1, width, rt.toast.text);
    rt.frame.set_draw_mode(DrawMode::Set);
    rt.frame.reset_clip();
}
