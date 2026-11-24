//! 菜单页：可滚动条目列表、虚拟返回项、隐禁状态与右侧取值列。

use crate::gfx::{DrawMode, Frame};
use crate::model::{InputAction, InputEvent, ItemState, MenuItem, MenuItemType, MenuPage, PageType};
use crate::runtime::{
    MENU_HEADER_HEIGHT, MENU_MS, PageState, ROW_HEIGHT, Runtime, Status, ease_q12, lerp_q12,
};
use crate::widgets::{dialog, utils};

/// 菜单内容区能完整显示的行数，屏越高行越多，至少一行。
fn visible_rows(rt: &Runtime) -> u16 {
    ((rt.frame.height() - MENU_HEADER_HEIGHT) / ROW_HEIGHT).max(1) as u16
}

/// 取当前页面对应的菜单页。
fn menu_page(rt: &Runtime, state: &PageState) -> &'static MenuPage {
    let route = utils::route_page(rt, state).expect("menu page route");
    &rt.app.menu_pages[route.index as usize]
}

/// 读取条目状态；没有状态表时一律正常。
fn menu_item_state(page: &MenuPage, index: u16) -> ItemState {
    let Some(states) = page.states else {
        return ItemState::Normal;
    };
    let packed = states[(index >> 2) as usize].get();
    match (packed >> ((index & 3) * 2)) & 3 {
        0 => ItemState::Normal,
        1 => ItemState::Hidden,
        2 => ItemState::Disabled,
        _ => ItemState::Reserved,
    }
}

/// 可见条目数（含虚拟返回项）。
fn menu_visible_count(page: &MenuPage) -> u16 {
    let mut count = 1;
    for index in 0..page.items.len() as u16 {
        if menu_item_state(page, index) != ItemState::Hidden {
            count += 1;
        }
    }
    count
}

/// 可选中条目数（含虚拟返回项）。
fn menu_selectable_count(page: &MenuPage) -> u16 {
    let mut count = 1;
    for index in 0..page.items.len() as u16 {
        if menu_item_state(page, index) == ItemState::Normal {
            count += 1;
        }
    }
    count
}

/// 可见序号转条目下标；虚拟返回项返回条目数。
fn menu_visible_to_item(page: &MenuPage, mut visible: u16) -> u16 {
    for index in 0..page.items.len() as u16 {
        if menu_item_state(page, index) == ItemState::Hidden {
            continue;
        }
        if visible == 0 {
            return index;
        }
        visible -= 1;
    }
    page.items.len() as u16
}

/// 条目下标转可见序号。
fn menu_item_to_visible(page: &MenuPage, item: u16) -> u16 {
    let mut visible = 0;
    for index in 0..page.items.len() as u16 {
        if index == item {
            return visible;
        }
        if menu_item_state(page, index) != ItemState::Hidden {
            visible += 1;
        }
    }
    visible
}

/// 条目是否可选中；虚拟返回项永远可选中。
fn menu_item_selectable(page: &MenuPage, item: u16) -> bool {
    item == page.items.len() as u16 || menu_item_state(page, item) == ItemState::Normal
}

/// 把焦点收缩到可选中条目，并同步滚动量。
pub(crate) fn menu_repair_focus(rt: &Runtime, state: &mut PageState) {
    let page = menu_page(rt, state);
    let count = page.items.len() as u16;
    if state.selected > count {
        state.selected = count;
    }
    let visible = if menu_item_selectable(page, state.selected) {
        menu_item_to_visible(page, state.selected)
    } else {
        let original = state.selected;
        state.selected = count;
        for index in original + 1..count {
            if menu_item_selectable(page, index) {
                state.selected = index;
                break;
            }
        }
        if state.selected == count {
            let mut index = original;
            while index > 0 {
                index -= 1;
                if menu_item_selectable(page, index) {
                    state.selected = index;
                    break;
                }
            }
        }
        menu_item_to_visible(page, state.selected)
    };
    if visible < state.top {
        state.top = visible;
    } else if visible >= state.top + visible_rows(rt) {
        state.top = visible - (visible_rows(rt) - 1);
    }
    let visible_count = menu_visible_count(page);
    let max_top = visible_count.saturating_sub(visible_rows(rt));
    state.top = state.top.min(max_top);
    state.scroll_q8 = state.top as i32 * ROW_HEIGHT as i32 * 256;
    state.focus_q8 = (visible - state.top) as i32 * ROW_HEIGHT as i32 * 256;
}

/// 当前（可能仍在动画中的）滚动量与焦点行，Q8。
fn menu_animated_values(rt: &Runtime, now: u32) -> (i32, i32) {
    if rt.list_anim.active {
        let p = ease_q12(
            now.wrapping_sub(rt.list_anim.started),
            rt.list_anim.duration,
        );
        (
            lerp_q12(rt.list_anim.scroll_from_q8, rt.current.scroll_q8, p),
            lerp_q12(rt.list_anim.focus_from_q8, rt.current.focus_q8, p),
        )
    } else {
        (rt.current.scroll_q8, rt.current.focus_q8)
    }
}

/// 菜单动画推进。
pub(crate) fn menu_animate(rt: &mut Runtime, now: u32) {
    if rt.current_page_type() != PageType::Menu || !rt.list_anim.active {
        return;
    }
    rt.dirty = true;
    if now.wrapping_sub(rt.list_anim.started) >= rt.list_anim.duration {
        rt.list_anim.active = false;
    }
}

/// 移动选中项。
fn menu_move(rt: &mut Runtime, direction: i16, steps: u16, now: u32) {
    let page = menu_page(rt, &rt.current);
    let count = menu_visible_count(page);
    let selectable = menu_selectable_count(page);
    let mut visible = menu_item_to_visible(page, rt.current.selected);
    if steps == 0 || selectable == 0 {
        return;
    }
    let (old_scroll, old_focus) = menu_animated_values(rt, now);
    let mut remaining = steps % selectable;
    if remaining == 0 {
        return;
    }
    while remaining != 0 {
        let item = loop {
            visible = if direction < 0 {
                if visible == 0 { count - 1 } else { visible - 1 }
            } else {
                (visible + 1) % count
            };
            let item = menu_visible_to_item(page, visible);
            if menu_item_selectable(page, item) {
                break item;
            }
        };
        rt.current.selected = item;
        remaining -= 1;
    }
    if visible < rt.current.top {
        rt.current.top = visible;
    } else if visible >= rt.current.top + visible_rows(rt) {
        rt.current.top = visible - (visible_rows(rt) - 1);
    }
    let max_top = count.saturating_sub(visible_rows(rt));
    rt.current.top = rt.current.top.min(max_top);
    rt.current.scroll_q8 = rt.current.top as i32 * 4096;
    rt.current.focus_q8 = (visible - rt.current.top) as i32 * 4096;
    rt.list_anim.scroll_from_q8 = old_scroll;
    rt.list_anim.focus_from_q8 = old_focus;
    rt.list_anim.started = now;
    rt.list_anim.duration = MENU_MS;
    rt.list_anim.active = true;
    rt.dirty = true;
}

/// 记录一次被拒绝的操作并闪边框。
fn menu_reject(rt: &mut Runtime, status: Status, index: u16) {
    rt.record_error(status, rt.current.page, index);
    rt.reject_flash = 2;
    rt.dirty = true;
}

/// 激活当前选中项。
fn menu_activate(rt: &mut Runtime, now: u32) {
    let page = menu_page(rt, &rt.current);
    let index = rt.current.selected;
    if index == page.items.len() as u16 {
        rt.navigate_back(now);
        return;
    }
    let item = &page.items[index as usize];
    let status = match item.kind {
        MenuItemType::Page => rt.navigate_to(item.reference, now),
        MenuItemType::Action => {
            if rt.queue_event(item.reference) {
                Status::Ok
            } else {
                Status::QueueFull
            }
        }
        MenuItemType::Int => dialog::open_int(rt, item.reference, now),
        MenuItemType::Bool => dialog::open_bool(rt, item.reference, now),
        MenuItemType::Confirm => dialog::open_confirm(rt, item.reference, now),
    };
    if status != Status::Ok && status != Status::Busy {
        menu_reject(rt, status, index);
    }
}

/// 菜单输入。
pub(crate) fn menu_input(rt: &mut Runtime, event: InputEvent, now: u32) {
    match event.action {
        InputAction::Ok => menu_activate(rt, now),
        InputAction::Up => menu_move(rt, -1, event.steps, now),
        InputAction::Down => menu_move(rt, 1, event.steps, now),
    }
}

/// 进入/返回箭头。
fn draw_enter_arrow(frame: &mut Frame<'_>, x: i16, y: i16, back: bool) {
    let point = x + if back { -3 } else { 3 };
    frame.draw_line(x, y + 5, point, y + 8);
    frame.draw_line(point, y + 8, x, y + 11);
}

/// 禁用标记：圆圈加斜杠。
fn draw_disabled(frame: &mut Frame<'_>, x: i16, y: i16) {
    frame.draw_circle(x, y, 4);
    frame.draw_line(x - 3, y + 3, x + 3, y - 3);
}

/// 条目右列的取值文本。
fn menu_value<'a>(rt: &Runtime, item: &MenuItem, buffer: &'a mut [u8; 20]) -> &'a str {
    match item.kind {
        MenuItemType::Int => {
            let binding = &rt.app.int_bindings[item.reference as usize];
            utils::format_int(binding.value.get(), binding.unit, buffer)
        }
        MenuItemType::Bool => {
            let binding = &rt.app.bool_bindings[item.reference as usize];
            if binding.value.get() {
                rt.app.texts.on_text
            } else {
                rt.app.texts.off_text
            }
        }
        _ => "",
    }
}

/// 条目内容区裁剪窗口。
fn menu_content_clip(
    rt: &mut Runtime,
    left: i16,
    right: i16,
    clip_x: i16,
    clip_right: i16,
    content_height: u16,
) {
    let left = left.max(clip_x);
    let right = right.min(clip_right);
    rt.frame.set_clip(
        left,
        MENU_HEADER_HEIGHT,
        (right - left).max(0) as u16,
        content_height,
    );
}

/// 绘制菜单页。
pub(crate) fn menu_draw(
    rt: &mut Runtime,
    state: PageState,
    is_current: bool,
    x_offset: i16,
    clip_x: i16,
    clip_width: u16,
) {
    let app = rt.app;
    let page = menu_page(rt, &state);
    let (scroll, focus) = if is_current && rt.list_anim.active {
        menu_animated_values(rt, rt.last_update)
    } else {
        (state.scroll_q8, state.focus_q8)
    };
    let visible_count = menu_visible_count(page);
    let clip_right = clip_x + clip_width as i16;
    let rows = visible_rows(rt);
    let content_height = rows * ROW_HEIGHT as u16;
    let screen_width = rt.frame.width();

    rt.frame
        .set_clip(clip_x, 0, clip_width, MENU_HEADER_HEIGHT as u16);
    utils::draw_centered(&mut rt.frame, x_offset, 1, screen_width as u16, page.title);

    rt.frame
        .set_clip(clip_x, MENU_HEADER_HEIGHT, clip_width, content_height);
    for visible in 0..visible_count {
        let item_index = menu_visible_to_item(page, visible);
        let y = MENU_HEADER_HEIGHT + visible as i16 * ROW_HEIGHT - (scroll / 256) as i16;
        if y >= MENU_HEADER_HEIGHT + rows as i16 * ROW_HEIGHT
            || y + ROW_HEIGHT <= MENU_HEADER_HEIGHT
        {
            continue;
        }
        if item_index == page.items.len() as u16 {
            menu_content_clip(
                rt,
                8 + x_offset,
                screen_width - 22 + x_offset,
                clip_x,
                clip_right,
                content_height,
            );
            rt.frame.draw_text(8 + x_offset, y, app.texts.return_text);
            rt.frame
                .set_clip(clip_x, MENU_HEADER_HEIGHT, clip_width, content_height);
            draw_enter_arrow(&mut rt.frame, screen_width - 18 + x_offset, y, true);
            continue;
        }
        let item = &page.items[item_index as usize];
        let mut buffer = [0u8; 20];
        let value = menu_value(rt, item, &mut buffer);
        if !value.is_empty() {
            // 取值文本要活着画完；借不到 &'static，先复制进局部缓冲再画。
            let mut text = [0u8; 20];
            let len = value.len().min(text.len());
            text[..len].copy_from_slice(&value.as_bytes()[..len]);
            let value = core::str::from_utf8(&text[..len]).unwrap_or("");
            let right = if menu_item_state(page, item_index) == ItemState::Normal {
                screen_width - 16
            } else {
                screen_width - 22
            };
            let value_x = right - rt.frame.text_width(value) as i16 + x_offset;
            menu_content_clip(
                rt,
                8 + x_offset,
                value_x - 2,
                clip_x,
                clip_right,
                content_height,
            );
            rt.frame.draw_text(8 + x_offset, y, item.label);
            rt.frame
                .set_clip(clip_x, MENU_HEADER_HEIGHT, clip_width, content_height);
            rt.frame.draw_text(value_x, y, value);
        } else if item.kind == MenuItemType::Page {
            menu_content_clip(
                rt,
                8 + x_offset,
                screen_width - 22 + x_offset,
                clip_x,
                clip_right,
                content_height,
            );
            rt.frame.draw_text(8 + x_offset, y, item.label);
            rt.frame
                .set_clip(clip_x, MENU_HEADER_HEIGHT, clip_width, content_height);
            draw_enter_arrow(&mut rt.frame, screen_width - 18 + x_offset, y, false);
        } else {
            let right = if menu_item_state(page, item_index) == ItemState::Normal {
                screen_width - 7
            } else {
                screen_width - 18
            };
            menu_content_clip(
                rt,
                8 + x_offset,
                right + x_offset,
                clip_x,
                clip_right,
                content_height,
            );
            rt.frame.draw_text(8 + x_offset, y, item.label);
            rt.frame
                .set_clip(clip_x, MENU_HEADER_HEIGHT, clip_width, content_height);
        }
        if menu_item_state(page, item_index) != ItemState::Normal {
            draw_disabled(&mut rt.frame, screen_width - 11 + x_offset, y + 8);
        }
    }
    rt.frame.set_draw_mode(DrawMode::Xor);
    rt.frame.draw_rbox(
        3 + x_offset,
        MENU_HEADER_HEIGHT + (focus / 256) as i16,
        (screen_width - 10) as u16,
        ROW_HEIGHT as u16,
        3,
    );
    rt.frame.set_draw_mode(DrawMode::Set);
    let track = content_height;
    if visible_count > rows
        && x_offset + screen_width - 4 < clip_right
        && x_offset + screen_width > clip_x
    {
        let first = ((scroll / 256) / ROW_HEIGHT as i32) as u16;
        let thumb_height = track * rows / visible_count;
        let maximum_first = visible_count - rows;
        let thumb_y = MENU_HEADER_HEIGHT + ((track - thumb_height) * first / maximum_first) as i16;
        rt.frame
            .draw_vline(screen_width - 3 + x_offset, MENU_HEADER_HEIGHT, track);
        rt.frame
            .draw_box(screen_width - 4 + x_offset, thumb_y, 3, thumb_height);
    }
    rt.frame.reset_clip();
}
