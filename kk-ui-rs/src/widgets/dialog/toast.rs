//! 贴底提示条：先滑入，到期后滑出。

use crate::gfx::DrawMode;
use crate::runtime::{Q12_ONE, Runtime, Status, TOAST_MS, ease_q12};

use crate::widgets::utils::draw_centered;

/// 打开 Toast；`duration` 为 0 时用默认时长。
pub(crate) fn open(rt: &mut Runtime, text: &'static str, duration: u32, now: u32) -> Status {
    if super::dialog_active(rt) {
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

/// Toast 动画：先滑入，到期后滑出。
pub(crate) fn animate(rt: &mut Runtime, now: u32) {
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

/// 绘制 Toast。
pub(crate) fn draw(rt: &mut Runtime<'_>) {
    if !rt.toast.visible || super::dialog_active(rt) {
        return;
    }
    let screen_width = rt.frame.width();
    let screen_height = rt.frame.height();
    let mut width = rt.frame.text_width(rt.toast.text) + 12;
    let limit = (screen_width - 8).max(0) as u16;
    if width > limit {
        width = limit;
    }
    if width & 1 != 0 {
        width += 1;
    }
    let x = (screen_width - width as i16) / 2;
    let y = screen_height - (18 * rt.toast.phase_q12 as i32 / Q12_ONE as i32) as i16;
    rt.frame.set_clip(x, y, width, 18);
    // 先清空整个包围盒，圆角外的像素才不会带着底下的页面字形。
    rt.frame.set_draw_mode(DrawMode::Clear);
    rt.frame.draw_box(x, y, width, 18);
    rt.frame.set_draw_mode(DrawMode::Set);
    rt.frame.draw_rbox(x, y, width, 18, 3);
    rt.frame.set_draw_mode(DrawMode::Clear);
    draw_centered(&mut rt.frame, x, y + 1, width, rt.toast.text);
    rt.frame.set_draw_mode(DrawMode::Set);
    rt.frame.reset_clip();
}
