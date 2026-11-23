//! 首页图标选择器、菜单与只读信息页。

use crate::dialog;
use crate::gfx::{DrawMode, Frame, HEIGHT, WIDTH};
use crate::model::*;
use crate::runtime::{
    HOME_MS, INFO_HEADER_HEIGHT, INFO_SCROLL_MS, MENU_HEADER_HEIGHT, MENU_MS, PageState,
    ROW_HEIGHT, Runtime, Status, VISIBLE_ROWS, ease_q12, lerp_q12, round_q8,
};

/// 菜单页「虚拟返回项」在可见序号里的位置等于条目数。
fn route_page(rt: &Runtime, state: &PageState) -> Option<PageRoute> {
    rt.route(state.page)
}

// ---------------------------------------------------------------- 首页

fn home_page(rt: &Runtime, state: &PageState) -> &'static HomePage {
    let route = route_page(rt, state).expect("home page route");
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
    let mut width = rt.frame.text_width(label) as i32 + 12;
    if width > 120 {
        width = 120;
    }
    width = (width + 1) & !1;
    width * 256
}

/// 相邻标签的横向间距。
fn home_text_spacing(rt: &mut Runtime, page: &'static HomePage) -> i16 {
    rt.frame.set_font(rt.app.fonts.home);
    let mut maximum = 0u16;
    for item in page.items {
        let width = rt.frame.text_width(item.label) + 12;
        maximum = maximum.max(width);
    }
    maximum.min(120) as i16 + 4
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

    rt.frame.set_clip(clip_x, 0, clip_width, HEIGHT as u16);
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
        let x = 48 + delta * 64 + icon_offset + x_offset;
        if x < clip_right && x + 32 > clip_x {
            rt.frame.draw_xbm(x, 8, 32, 32, item.icon);
        }
    }

    let label_left = 64 - label_width / 2 + x_offset;
    let left = label_left.max(clip_x);
    let right = (label_left + label_width).min(clip_right);
    if right > left {
        rt.frame.set_clip(left, 44, (right - left) as u16, 20);
        for (index, item) in page.items.iter().enumerate() {
            let delta = home_delta(index, state.selected, page.items.len() as u16);
            let text_width = rt.frame.text_width(item.label) as i16;
            let text_x =
                (WIDTH - text_width) / 2 + delta * text_spacing + text_offset as i16 + x_offset;
            rt.frame.draw_text(text_x, 46, item.label);
        }
        rt.frame.set_draw_mode(DrawMode::Xor);
        rt.frame
            .draw_rbox(label_left, 44, label_width as u16, 20, 3);
        rt.frame.set_draw_mode(DrawMode::Set);
    }
    rt.frame.reset_clip();
}

// ---------------------------------------------------------------- 菜单

fn menu_page(rt: &Runtime, state: &PageState) -> &'static MenuPage {
    let route = route_page(rt, state).expect("menu page route");
    &rt.app.menu_pages[route.index as usize]
}

/// 读取条目状态；没有状态表时一律正常。
pub(crate) fn menu_item_state(page: &MenuPage, index: u16) -> ItemState {
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
    } else if visible >= state.top + VISIBLE_ROWS {
        state.top = visible - (VISIBLE_ROWS - 1);
    }
    let visible_count = menu_visible_count(page);
    let max_top = visible_count.saturating_sub(VISIBLE_ROWS);
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
    } else if visible >= rt.current.top + VISIBLE_ROWS {
        rt.current.top = visible - (VISIBLE_ROWS - 1);
    }
    let max_top = count.saturating_sub(VISIBLE_ROWS);
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
fn draw_enter_arrow(frame: &mut Frame, x: i16, y: i16, back: bool) {
    let point = x + if back { -3 } else { 3 };
    frame.draw_line(x, y + 5, point, y + 8);
    frame.draw_line(point, y + 8, x, y + 11);
}

/// 禁用标记：圆圈加斜杠。
fn draw_disabled(frame: &mut Frame, x: i16, y: i16) {
    frame.draw_circle(x, y, 4);
    frame.draw_line(x - 3, y + 3, x + 3, y - 3);
}

/// 条目右列的取值文本。
fn menu_value<'a>(rt: &Runtime, item: &MenuItem, buffer: &'a mut [u8; 20]) -> &'a str {
    match item.kind {
        MenuItemType::Int => {
            let binding = &rt.app.int_bindings[item.reference as usize];
            dialog::format_int(binding.value.get(), binding.unit, buffer)
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
fn menu_content_clip(rt: &mut Runtime, left: i16, right: i16, clip_x: i16, clip_right: i16) {
    let left = left.max(clip_x);
    let right = right.min(clip_right);
    rt.frame.set_clip(
        left,
        MENU_HEADER_HEIGHT,
        (right - left).max(0) as u16,
        VISIBLE_ROWS * ROW_HEIGHT as u16,
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
    let content_height = VISIBLE_ROWS * ROW_HEIGHT as u16;

    rt.frame.set_font(app.fonts.title);
    rt.frame
        .set_clip(clip_x, 0, clip_width, MENU_HEADER_HEIGHT as u16);
    dialog::draw_centered(&mut rt.frame, x_offset, 1, WIDTH as u16, page.title);

    rt.frame.set_font(app.fonts.body);
    rt.frame
        .set_clip(clip_x, MENU_HEADER_HEIGHT, clip_width, content_height);
    for visible in 0..visible_count {
        let item_index = menu_visible_to_item(page, visible);
        let y = MENU_HEADER_HEIGHT + visible as i16 * ROW_HEIGHT - (scroll / 256) as i16;
        if y >= MENU_HEADER_HEIGHT + VISIBLE_ROWS as i16 * ROW_HEIGHT
            || y + ROW_HEIGHT <= MENU_HEADER_HEIGHT
        {
            continue;
        }
        if item_index == page.items.len() as u16 {
            menu_content_clip(rt, 8 + x_offset, 106 + x_offset, clip_x, clip_right);
            rt.frame.draw_text(8 + x_offset, y, app.texts.return_text);
            rt.frame
                .set_clip(clip_x, MENU_HEADER_HEIGHT, clip_width, content_height);
            draw_enter_arrow(&mut rt.frame, 110 + x_offset, y, true);
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
                112
            } else {
                106
            };
            let value_x = right - rt.frame.text_width(value) as i16 + x_offset;
            menu_content_clip(rt, 8 + x_offset, value_x - 2, clip_x, clip_right);
            rt.frame.draw_text(8 + x_offset, y, item.label);
            rt.frame
                .set_clip(clip_x, MENU_HEADER_HEIGHT, clip_width, content_height);
            rt.frame.draw_text(value_x, y, value);
        } else if item.kind == MenuItemType::Page {
            menu_content_clip(rt, 8 + x_offset, 106 + x_offset, clip_x, clip_right);
            rt.frame.draw_text(8 + x_offset, y, item.label);
            rt.frame
                .set_clip(clip_x, MENU_HEADER_HEIGHT, clip_width, content_height);
            draw_enter_arrow(&mut rt.frame, 110 + x_offset, y, false);
        } else {
            let right = if menu_item_state(page, item_index) == ItemState::Normal {
                121
            } else {
                110
            };
            menu_content_clip(rt, 8 + x_offset, right + x_offset, clip_x, clip_right);
            rt.frame.draw_text(8 + x_offset, y, item.label);
            rt.frame
                .set_clip(clip_x, MENU_HEADER_HEIGHT, clip_width, content_height);
        }
        if menu_item_state(page, item_index) != ItemState::Normal {
            draw_disabled(&mut rt.frame, 117 + x_offset, y + 8);
        }
    }
    rt.frame.set_draw_mode(DrawMode::Xor);
    rt.frame.draw_rbox(
        3 + x_offset,
        MENU_HEADER_HEIGHT + (focus / 256) as i16,
        118,
        ROW_HEIGHT as u16,
        3,
    );
    rt.frame.set_draw_mode(DrawMode::Set);
    if visible_count > VISIBLE_ROWS && x_offset + 124 < clip_right && x_offset + 127 >= clip_x {
        let first = ((scroll / 256) / ROW_HEIGHT as i32) as u16;
        let thumb_height = 48 * VISIBLE_ROWS / visible_count;
        let maximum_first = visible_count - VISIBLE_ROWS;
        let thumb_y = MENU_HEADER_HEIGHT + ((48 - thumb_height) * first / maximum_first) as i16;
        rt.frame.draw_vline(125 + x_offset, MENU_HEADER_HEIGHT, 48);
        rt.frame.draw_box(124 + x_offset, thumb_y, 3, thumb_height);
    }
    rt.frame.reset_clip();
}

// ---------------------------------------------------------------- 信息页

fn info_page(rt: &Runtime, state: &PageState) -> &'static InfoPage {
    let route = route_page(rt, state).expect("info page route");
    &rt.app.info_pages[route.index as usize]
}

fn info_header_height(page: &InfoPage) -> i16 {
    if page.title.is_empty() {
        0
    } else {
        INFO_HEADER_HEIGHT
    }
}

fn info_max_scroll_q8(page: &InfoPage) -> i32 {
    let header = info_header_height(page);
    let content = page.rows.len() as i32 * ROW_HEIGHT as i32;
    let viewport = HEIGHT as i32 - header as i32;
    if content > viewport {
        (content - viewport) * 256
    } else {
        0
    }
}

fn info_scroll(rt: &Runtime, now: u32) -> i32 {
    if rt.list_anim.active {
        let p = ease_q12(
            now.wrapping_sub(rt.list_anim.started),
            rt.list_anim.duration,
        );
        lerp_q12(rt.list_anim.scroll_from_q8, rt.current.scroll_q8, p)
    } else {
        rt.current.scroll_q8
    }
}

/// 信息页动画推进。
pub(crate) fn info_animate(rt: &mut Runtime, now: u32) {
    if rt.current_page_type() != PageType::Info || !rt.list_anim.active {
        return;
    }
    rt.dirty = true;
    if now.wrapping_sub(rt.list_anim.started) >= rt.list_anim.duration {
        rt.list_anim.active = false;
    }
}

/// 滚动信息页。
fn info_move(rt: &mut Runtime, direction: i16, steps: u16, now: u32) {
    let page = info_page(rt, &rt.current);
    let current = info_scroll(rt, now);
    let maximum = info_max_scroll_q8(page);
    if steps == 0 || maximum == 0 {
        return;
    }
    let amount = steps as i32 * ROW_HEIGHT as i32 * 256;
    let target = rt.current.scroll_q8;
    let target = if direction < 0 {
        if target > amount { target - amount } else { 0 }
    } else if maximum - target > amount {
        target + amount
    } else {
        maximum
    };
    if target == rt.current.scroll_q8 {
        return;
    }
    rt.current.scroll_q8 = target;
    rt.current.top = (target / (ROW_HEIGHT as i32 * 256)) as u16;
    rt.list_anim.scroll_from_q8 = current;
    rt.list_anim.focus_from_q8 = 0;
    rt.list_anim.started = now;
    rt.list_anim.duration = INFO_SCROLL_MS;
    rt.list_anim.active = true;
    rt.dirty = true;
}

/// 信息页输入。
pub(crate) fn info_input(rt: &mut Runtime, event: InputEvent, now: u32) {
    match event.action {
        InputAction::Ok => {
            rt.navigate_back(now);
        }
        InputAction::Up => info_move(rt, -1, event.steps, now),
        InputAction::Down => info_move(rt, 1, event.steps, now),
    }
}

/// 右上角关闭图标。
fn info_close_icon(frame: &mut Frame, x_offset: i16) {
    frame.draw_rbox(114 + x_offset, 1, 12, 12, 2);
    frame.set_draw_mode(DrawMode::Clear);
    frame.draw_line(117 + x_offset, 4, 122 + x_offset, 9);
    frame.draw_line(122 + x_offset, 4, 117 + x_offset, 9);
    frame.set_draw_mode(DrawMode::Set);
}

/// 绘制信息页。
pub(crate) fn info_draw(
    rt: &mut Runtime,
    state: PageState,
    is_current: bool,
    x_offset: i16,
    clip_x: i16,
    clip_width: u16,
) {
    let app = rt.app;
    let page = info_page(rt, &state);
    let header = info_header_height(page);
    let scroll = if is_current && rt.list_anim.active {
        info_scroll(rt, rt.last_update)
    } else {
        state.scroll_q8
    };
    let visible_rows = ((HEIGHT - header) / ROW_HEIGHT) as u16;
    let clip_right = clip_x + clip_width as i16;
    let content_left = clip_x;
    let mut content_right = clip_right;
    let mut draw_content = true;

    if header != 0 {
        rt.frame.set_font(app.fonts.title);
        rt.frame.set_clip(clip_x, 0, clip_width, header as u16);
        dialog::draw_centered(&mut rt.frame, x_offset, 1, WIDTH as u16, page.title);
        rt.frame.draw_hline(x_offset, header - 1, 125);
        info_close_icon(&mut rt.frame, x_offset);
    }
    rt.frame.set_font(app.fonts.body);
    if content_right > x_offset + 125 {
        content_right = x_offset + 125;
    }
    let content_left = content_left.max(x_offset);
    if content_right > content_left {
        rt.frame.set_clip(
            content_left,
            header,
            (content_right - content_left) as u16,
            (HEIGHT - header) as u16,
        );
    } else {
        draw_content = false;
    }
    for (index, row) in page.rows.iter().enumerate() {
        let y = header + index as i16 * ROW_HEIGHT - round_q8(scroll);
        if !draw_content || y >= HEIGHT || y + ROW_HEIGHT <= header {
            continue;
        }
        let has_value = row.value.with(|value| !value.is_empty());
        if has_value {
            let value_width = row.value.with(|value| rt.frame.text_width(value)) as i16;
            let value_x = 121 - value_width + x_offset;
            let name_left = (6 + x_offset).max(content_left);
            let name_right = (value_x - 2).min(content_right);
            if name_right > name_left {
                rt.frame.set_clip(
                    name_left,
                    header,
                    (name_right - name_left) as u16,
                    (HEIGHT - header) as u16,
                );
                rt.frame.draw_text(6 + x_offset, y, row.name);
            }
            rt.frame.set_clip(
                content_left,
                header,
                (content_right - content_left) as u16,
                (HEIGHT - header) as u16,
            );
            row.value.with(|value| {
                rt.frame.draw_text(value_x, y, value);
            });
        } else {
            rt.frame.draw_text(6 + x_offset, y, row.name);
        }
    }
    rt.frame.set_clip(clip_x, 0, clip_width, HEIGHT as u16);
    if page.rows.len() as u16 > visible_rows
        && x_offset + 126 >= clip_x
        && x_offset + 126 < clip_right
    {
        let viewport = HEIGHT - header;
        let mut thumb_height = (viewport as u32 * viewport as u32
            / (page.rows.len() as u32 * ROW_HEIGHT as u32)) as i16;
        if thumb_height < 8 {
            thumb_height = 8;
        }
        let maximum = info_max_scroll_q8(page) / 256;
        let scroll_pixels = round_q8(scroll) as i32;
        let thumb_y =
            header + ((viewport as i32 - thumb_height as i32) * scroll_pixels / maximum) as i16;
        rt.frame
            .draw_box(126 + x_offset, thumb_y, 2, thumb_height as u16);
    }
    rt.frame.reset_clip();
}

/// 让编译器确认未使用的导入仍属有意保留。
#[allow(dead_code)]
fn _unused(_: Status) {}
