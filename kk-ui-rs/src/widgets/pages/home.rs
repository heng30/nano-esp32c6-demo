//! 首页：环形排列的图标选择器，切换时横向滑入。

use crate::gfx::DrawMode;
use crate::model::{HomePage, InputAction, InputEvent, PageType};
use crate::runtime::{HOME_MS, PageState, Runtime, ease_q12, lerp_q12, round_q8};

/// 首页标签框的宽度上限：画布宽度留 4 px 边距。
fn home_label_limit(rt: &Runtime) -> i32 {
    rt.frame.width() as i32 - 8
}

/// 取当前页面对应的首页入口表。
fn home_page(rt: &Runtime, state: &PageState) -> &'static HomePage {
    let route = rt.route(state.page).expect("home page route");
    &rt.app.home_pages[route.index as usize]
}

/// 环形展开后的相对位置。
fn home_delta(index: usize, selected: u16, count: u16) -> i16 {
    let mut delta = index as i32 - selected as i32;
    let half = (count / 2) as i32;
    if delta > half {
        delta -= count as i32;
    } else if delta < -half {
        delta += count as i32;
    }
    delta as i16
}

/// 选中标签的高亮框宽度，Q8。
fn home_label_width_q8(rt: &mut Runtime, page: &'static HomePage, selected: u16) -> i32 {
    rt.frame.set_font(rt.app.fonts.home);
    let label = page.items[selected as usize].label;
    let limit = home_label_limit(rt);
    let mut width = rt.frame.text_width(label) as i32 + 12;
    if width > limit {
        width = limit;
    }
    width = (width + 1) & !1;
    width * 256
}

/// 相邻标签的横向间距。
fn home_text_spacing(rt: &mut Runtime, page: &'static HomePage) -> i16 {
    rt.frame.set_font(rt.app.fonts.home);
    let limit = home_label_limit(rt).max(0) as u16;
    let mut maximum = 0u16;
    for item in page.items {
        let width = rt.frame.text_width(item.label) + 12;
        maximum = maximum.max(width);
    }
    maximum.min(limit) as i16 + 4
}

/// 当前（可能仍在动画中的）横向偏移与标签宽度，Q8。
fn home_values(rt: &mut Runtime, now: u32) -> (i32, i32) {
    let page = home_page(rt, &rt.current);
    if rt.home_anim.active {
        let p = ease_q12(now.wrapping_sub(rt.home_anim.started), HOME_MS);
        (
            lerp_q12(rt.home_anim.offset_from_q8, 0, p),
            lerp_q12(rt.home_anim.label_from_q8, rt.home_anim.label_to_q8, p),
        )
    } else {
        (0, home_label_width_q8(rt, page, rt.current.selected))
    }
}

/// 首页动画推进。
pub(crate) fn home_animate(rt: &mut Runtime, now: u32) {
    if rt.current_page_type() != PageType::Home || !rt.home_anim.active {
        return;
    }
    rt.dirty = true;
    if now.wrapping_sub(rt.home_anim.started) >= HOME_MS {
        rt.home_anim.active = false;
    }
}

/// 切换选中项。
fn home_move(rt: &mut Runtime, direction: i16, steps: u16, now: u32) {
    let page = home_page(rt, &rt.current);
    let count = page.items.len() as u16;
    if count == 0 || steps == 0 {
        return;
    }
    let (current_offset, current_label) = home_values(rt, now);
    let mut step = (steps % count) as i32;
    if step == 0 {
        return;
    }
    if direction < 0 {
        step = -step;
    }
    rt.current.selected =
        ((rt.current.selected as i32 + step + count as i32 * 2) % count as i32) as u16;
    rt.home_anim.offset_from_q8 = current_offset + direction as i32 * 64 * 256;
    rt.home_anim.label_from_q8 = current_label;
    rt.home_anim.label_to_q8 = home_label_width_q8(rt, page, rt.current.selected);
    rt.home_anim.started = now;
    rt.home_anim.active = true;
    rt.dirty = true;
}

/// 首页输入。
pub(crate) fn home_input(rt: &mut Runtime, event: InputEvent, now: u32) {
    let page = home_page(rt, &rt.current);
    match event.action {
        InputAction::Ok => {
            let target = page.items[rt.current.selected as usize].target;
            rt.home_anim.active = false;
            rt.navigate_to(target, now);
        }
        InputAction::Up => home_move(rt, -1, event.steps, now),
        InputAction::Down => home_move(rt, 1, event.steps, now),
    }
}

/// 绘制首页。
pub(crate) fn home_draw(
    rt: &mut Runtime,
    state: PageState,
    is_current: bool,
    x_offset: i16,
    clip_x: i16,
    clip_width: u16,
) {
    let app = rt.app;
    let page = home_page(rt, &state);
    let clip_right = clip_x + clip_width as i16;
    let (screen_width, screen_height) = (rt.frame.width(), rt.frame.height());
    // 首页内容按 64 行高的基准布局，画布更高时整块垂直居中。
    let dy = (screen_height - 64) / 2;

    rt.frame
        .set_clip(clip_x, 0, clip_width, screen_height as u16);
    rt.frame.set_font(app.fonts.home);
    let (icon_offset_q8, label_width_q8) = if is_current && rt.home_anim.active {
        let p = ease_q12(rt.last_update.wrapping_sub(rt.home_anim.started), HOME_MS);
        (
            lerp_q12(rt.home_anim.offset_from_q8, 0, p),
            lerp_q12(rt.home_anim.label_from_q8, rt.home_anim.label_to_q8, p),
        )
    } else {
        (0, home_label_width_q8(rt, page, state.selected))
    };
    let icon_offset = round_q8(icon_offset_q8);
    let label_width = round_q8(label_width_q8);
    let text_spacing = home_text_spacing(rt, page);
    let text_offset = icon_offset as i32 * text_spacing as i32 / 64;

    for (index, item) in page.items.iter().enumerate() {
        let delta = home_delta(index, state.selected, page.items.len() as u16);
        let x = screen_width / 2 - 16 + delta * 64 + icon_offset + x_offset;
        if x < clip_right && x + 32 > clip_x {
            rt.frame.draw_xbm(x, 8 + dy, 32, 32, item.icon);
        }
    }

    let label_left = screen_width / 2 - label_width / 2 + x_offset;
    let left = label_left.max(clip_x);
    let right = (label_left + label_width).min(clip_right);
    if right > left {
        rt.frame.set_clip(left, 44 + dy, (right - left) as u16, 20);
        for (index, item) in page.items.iter().enumerate() {
            let delta = home_delta(index, state.selected, page.items.len() as u16);
            let text_width = rt.frame.text_width(item.label) as i16;
            let text_x = (screen_width - text_width) / 2
                + delta * text_spacing
                + text_offset as i16
                + x_offset;
            rt.frame.draw_text(text_x, 46 + dy, item.label);
        }
        rt.frame.set_draw_mode(DrawMode::Xor);
        rt.frame
            .draw_rbox(label_left, 44 + dy, label_width as u16, 20, 3);
        rt.frame.set_draw_mode(DrawMode::Set);
    }
    rt.frame.reset_clip();
}
