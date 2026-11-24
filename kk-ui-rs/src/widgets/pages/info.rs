//! 只读信息页：名称/取值两列，超出视口时滚动。

use crate::gfx::{DrawMode, Frame};
use crate::model::{InfoPage, InputAction, InputEvent, PageType};
use crate::runtime::{
    INFO_HEADER_HEIGHT, INFO_SCROLL_MS, PageState, ROW_HEIGHT, Runtime, ease_q12, lerp_q12,
    round_q8,
};
use crate::widgets::utils;

/// 取当前页面对应的信息页。
fn info_page(rt: &Runtime, state: &PageState) -> &'static InfoPage {
    let route = utils::route_page(rt, state).expect("info page route");
    &rt.app.info_pages[route.index as usize]
}

/// 信息页标题栏高度；无标题时为 0。
fn info_header_height(page: &InfoPage) -> i16 {
    if page.title.is_empty() {
        0
    } else {
        INFO_HEADER_HEIGHT
    }
}

/// 信息页可滚动的最大距离，Q8；内容装得下时为 0。
fn info_max_scroll_q8(rt: &Runtime, page: &InfoPage) -> i32 {
    let header = info_header_height(page);
    let content = page.rows.len() as i32 * ROW_HEIGHT as i32;
    let viewport = rt.frame.height() as i32 - header as i32;
    if content > viewport {
        (content - viewport) * 256
    } else {
        0
    }
}

/// 当前（可能仍在动画中的）滚动量，Q8。
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
    let maximum = info_max_scroll_q8(rt, page);
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
fn info_close_icon(frame: &mut Frame<'_>, x_offset: i16) {
    let x = frame.width() - 14 + x_offset;
    frame.draw_rbox(x, 1, 12, 12, 2);
    frame.set_draw_mode(DrawMode::Clear);
    frame.draw_line(x + 3, 4, x + 8, 9);
    frame.draw_line(x + 8, 4, x + 3, 9);
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
    let (screen_width, screen_height) = (rt.frame.width(), rt.frame.height());
    let scroll = if is_current && rt.list_anim.active {
        info_scroll(rt, rt.last_update)
    } else {
        state.scroll_q8
    };
    let visible_rows = ((screen_height - header) / ROW_HEIGHT) as u16;
    let clip_right = clip_x + clip_width as i16;
    let content_left = clip_x;
    let mut content_right = clip_right;
    let mut draw_content = true;

    if header != 0 {
        rt.frame.set_font(app.fonts.title);
        rt.frame.set_clip(clip_x, 0, clip_width, header as u16);
        utils::draw_centered(&mut rt.frame, x_offset, 1, screen_width as u16, page.title);
        rt.frame
            .draw_hline(x_offset, header - 1, (screen_width - 3) as u16);
        info_close_icon(&mut rt.frame, x_offset);
    }
    rt.frame.set_font(app.fonts.body);
    if content_right > x_offset + screen_width - 3 {
        content_right = x_offset + screen_width - 3;
    }
    let content_left = content_left.max(x_offset);
    if content_right > content_left {
        rt.frame.set_clip(
            content_left,
            header,
            (content_right - content_left) as u16,
            (screen_height - header) as u16,
        );
    } else {
        draw_content = false;
    }
    for (index, row) in page.rows.iter().enumerate() {
        let y = header + index as i16 * ROW_HEIGHT - round_q8(scroll);
        if !draw_content || y >= screen_height || y + ROW_HEIGHT <= header {
            continue;
        }
        let has_value = row.value.with(|value| !value.is_empty());
        if has_value {
            let value_width = row.value.with(|value| rt.frame.text_width(value)) as i16;
            let value_x = screen_width - 7 - value_width + x_offset;
            let name_left = (6 + x_offset).max(content_left);
            let name_right = (value_x - 2).min(content_right);
            if name_right > name_left {
                rt.frame.set_clip(
                    name_left,
                    header,
                    (name_right - name_left) as u16,
                    (screen_height - header) as u16,
                );
                rt.frame.draw_text(6 + x_offset, y, row.name);
            }
            rt.frame.set_clip(
                content_left,
                header,
                (content_right - content_left) as u16,
                (screen_height - header) as u16,
            );
            row.value.with(|value| {
                rt.frame.draw_text(value_x, y, value);
            });
        } else {
            rt.frame.draw_text(6 + x_offset, y, row.name);
        }
    }
    rt.frame
        .set_clip(clip_x, 0, clip_width, screen_height as u16);
    if page.rows.len() as u16 > visible_rows
        && x_offset + screen_width > clip_x
        && x_offset + screen_width - 2 < clip_right
    {
        let viewport = screen_height - header;
        let mut thumb_height = (viewport as u32 * viewport as u32
            / (page.rows.len() as u32 * ROW_HEIGHT as u32)) as i16;
        if thumb_height < 8 {
            thumb_height = 8;
        }
        let maximum = info_max_scroll_q8(rt, page) / 256;
        let scroll_pixels = round_q8(scroll) as i32;
        let thumb_y =
            header + ((viewport as i32 - thumb_height as i32) * scroll_pixels / maximum) as i16;
        rt.frame
            .draw_box(screen_width - 2 + x_offset, thumb_y, 2, thumb_height as u16);
    }
    rt.frame.reset_clip();
}
