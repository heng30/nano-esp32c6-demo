//! 弹窗与 Toast 的公共骨架：会话状态、开合动画、焦点补间与输入/绘制分发。
//!
//! 一种弹窗一个文件：`int_editor`、`bool_editor`、`confirm`、`message`；贴底提示条在
//! `toast`，共用的文本绘制与整数格式化在 `text`。这里只保留与弹窗类型无关的部分。

mod bool_editor;
mod confirm;
mod int_editor;
mod message;
mod toast;

use crate::gfx::{DrawMode, Frame};
use crate::model::{EVENT_NONE, EventId, InputAction, InputEvent, PageType};
use crate::widgets::utils;
use crate::runtime::{
    BOOL_VALUE_MS, DIALOG_CLOSE_MS, DIALOG_OPEN_MS, DialogFocus, FOCUS_MS, INT_VALUE_MS,
    MENU_HEADER_HEIGHT, OverlayType, Q12_ONE, Runtime, Status, ease_q12, lerp_q12, round_q8,
};

pub(crate) use bool_editor::open as open_bool;
pub(crate) use confirm::open as open_confirm;
pub(crate) use int_editor::open as open_int;
pub(crate) use message::open as open_message;
pub(crate) use toast::draw as draw_toast;
pub(crate) use toast::open as toast_open;

/// 弹窗内容相对画布左侧的偏移：128 px 画布上为 3，与 C 版一致。
fn dialog_left(rt: &Runtime) -> i16 {
    (rt.frame.width() - 122) / 2
}

/// 弹窗内容相对画布顶部的偏移：按 64 行高的基准布局，画布更高时垂直居中。
fn dialog_top(rt: &Runtime) -> i16 {
    (rt.frame.height() - 64) / 2
}

/// 弹窗出现时对齐的行号：菜单页对齐当前焦点行，其它页取屏幕中部。
fn dialog_origin(rt: &Runtime) -> i16 {
    if rt.current_page_type() == PageType::Menu {
        MENU_HEADER_HEIGHT + round_q8(rt.current.focus_q8)
    } else {
        dialog_top(rt) + 24
    }
}

/// 是否有覆盖层占用输入。
pub(crate) fn dialog_active(rt: &Runtime) -> bool {
    rt.overlay.kind != OverlayType::None
}

/// 焦点框的目标矩形。
fn focus_rect(rt: &Runtime, focus: DialogFocus) -> (i16, i16, i16, i16) {
    let dx = dialog_left(rt);
    let dy = dialog_top(rt);
    if rt.overlay.kind == OverlayType::Message {
        (46 + dx, 41 + dy, 36, 16)
    } else if focus == DialogFocus::Value {
        let bool_editor = rt.overlay.kind == OverlayType::Bool;
        (
            if bool_editor { 29 } else { 39 } + dx,
            21 + dy,
            if bool_editor { 70 } else { 46 },
            16,
        )
    } else if focus == DialogFocus::Confirm {
        (77 + dx, 41 + dy, 36, 16)
    } else {
        (15 + dx, 41 + dy, 36, 16)
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
    rt.overlay.focus_mode = focus;
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

/// 当前帧弹窗的几何：内容偏移 `(dx, dy)` 与动画中的外框 `(x, y, width, height)`。
pub(crate) struct Layout {
    /// 内容相对画布左侧的偏移。
    pub dx: i16,
    /// 内容相对画布顶部的偏移。
    pub dy: i16,
    /// 外框左边。
    pub x: i16,
    /// 外框上边。
    pub y: i16,
    /// 外框宽度。
    pub width: i16,
    /// 外框高度。
    pub height: i16,
}

impl Layout {
    /// 把裁剪窗口收进外框内部 1 px。
    pub(crate) fn apply_clip(&self, frame: &mut Frame<'_>) {
        frame.set_clip(
            self.x + 1,
            self.y + 1,
            (self.width - 2) as u16,
            (self.height - 2) as u16,
        );
    }
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

/// 开始关闭动画。
pub(crate) fn close(rt: &mut Runtime, now: u32) {
    rt.overlay.phase_from_q12 = rt.overlay.phase_q12;
    rt.overlay.phase_started = now;
    rt.overlay.closing = true;
    rt.overlay.editing = false;
    rt.dirty = true;
}

/// 入队事件；队列满时记录错误并闪一下边框。
pub(crate) fn event_or_reject(rt: &mut Runtime, event: EventId) -> bool {
    if event == EVENT_NONE || rt.queue_event(event) {
        return true;
    }
    rt.record_error(Status::QueueFull, rt.current.page, event);
    rt.reject_flash = 2;
    rt.dirty = true;
    false
}

/// 写入新草稿并启动换值动画。
fn apply_value(rt: &mut Runtime, next: i32, increase: bool, now: u32) {
    if next == rt.overlay.draft {
        return;
    }
    rt.overlay.previous = rt.overlay.draft;
    rt.overlay.draft = next;
    rt.overlay.value_direction = if increase { 1 } else { -1 };
    rt.overlay.value_q12 = 0;
    rt.overlay.value_started = now;
    rt.overlay.value_moving = true;
    rt.dirty = true;
}

/// 弹窗标题与标题下的分隔线。
pub(crate) fn draw_title(rt: &mut Runtime<'_>, layout: &Layout, title: &str) {
    utils::draw_centered(&mut rt.frame, 10 + layout.dx, 3 + layout.dy, 108, title);
    rt.frame
        .draw_hline(12 + layout.dx, 19 + layout.dy, 104);
}

/// 底部按钮行；`cancel` 为 `false` 时只画居中的确定键。
pub(crate) fn draw_buttons(rt: &mut Runtime<'_>, layout: &Layout, cancel: bool) {
    layout.apply_clip(&mut rt.frame);
    if cancel {
        utils::draw_centered(
            &mut rt.frame,
            15 + layout.dx,
            41 + layout.dy,
            36,
            rt.app.texts.cancel_text,
        );
        utils::draw_centered(
            &mut rt.frame,
            77 + layout.dx,
            41 + layout.dy,
            36,
            rt.app.texts.confirm_text,
        );
    } else {
        utils::draw_centered(
            &mut rt.frame,
            46 + layout.dx,
            41 + layout.dy,
            36,
            rt.app.texts.confirm_text,
        );
    }
}

/// 处理输入。
pub(crate) fn input(rt: &mut Runtime, event: InputEvent, now: u32) {
    if !dialog_active(rt) || rt.overlay.closing {
        return;
    }
    match rt.overlay.kind {
        OverlayType::Message => message::input(rt, event, now),
        OverlayType::Confirm => confirm::input(rt, event, now),
        OverlayType::Int | OverlayType::Bool => editor_input(rt, event, now),
        OverlayType::None => {}
    }
}

/// 两种编辑器共用的输入状态机：取值态改值，按钮态提交或取消。
fn editor_input(rt: &mut Runtime, event: InputEvent, now: u32) {
    if rt.overlay.editing {
        if event.action == InputAction::Ok {
            rt.overlay.editing = false;
            set_focus(rt, DialogFocus::Confirm, now);
        } else if rt.overlay.kind == OverlayType::Int {
            int_editor::change(rt, event, now);
        } else {
            bool_editor::change(rt, event, now);
        }
        return;
    }
    if event.action == InputAction::Ok {
        match rt.overlay.focus_mode {
            DialogFocus::Value => {
                rt.overlay.editing = true;
                rt.dirty = true;
            }
            DialogFocus::Confirm => {
                if rt.overlay.kind == OverlayType::Int {
                    int_editor::commit(rt, now);
                } else {
                    bool_editor::commit(rt, now);
                }
            }
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
    toast::animate(rt, now);
}

/// 绘制覆盖层。
pub(crate) fn draw(rt: &mut Runtime<'_>) {
    if !dialog_active(rt) {
        return;
    }
    let p = rt.overlay.phase_q12 as i32;
    let one = Q12_ONE as i32;
    let dx = dialog_left(rt);
    let dy = dialog_top(rt);
    let screen_width = rt.frame.width();
    let target_y = dy as i32 + 1;
    let x = (dx as i32 + 3 * p / one) as i16;
    let y =
        (rt.overlay.origin_y as i32 + (target_y - rt.overlay.origin_y as i32) * p / one) as i16;
    let width = (118 - 2 * p / one).min(screen_width as i32 - 2 * dx as i32 + 1) as i16;
    let height = (16 + 45 * p / one) as i16;
    {
        let frame = &mut rt.frame;
        frame.reset_clip();
        frame.set_draw_mode(DrawMode::Clear);
        frame.draw_rbox(x, y, width as u16, height as u16, 3);
        frame.set_draw_mode(DrawMode::Set);
        frame.draw_rframe(x, y, width as u16, height as u16, 3);
    }
    if width <= 2 || height <= 2 {
        return;
    }
    let layout = Layout {
        dx,
        dy,
        x,
        y,
        width,
        height,
    };
    layout.apply_clip(&mut rt.frame);
    rt.frame.set_font(rt.app.fonts.body);
    match rt.overlay.kind {
        OverlayType::Confirm => confirm::draw(rt, &layout),
        OverlayType::Int => int_editor::draw(rt, &layout),
        OverlayType::Bool => bool_editor::draw(rt, &layout),
        OverlayType::Message => message::draw(rt, &layout),
        OverlayType::None => {}
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
