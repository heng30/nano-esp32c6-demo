//! 正弦自绘页：带坐标轴的正弦曲线，随时间向左移动。

use kk_ui::{CustomPageHandler, Frame, InputAction, InputEvent, PageRoute, Runtime, Shared};

use super::tables::APP_NOW;

/// 正弦页的帧间状态。
#[derive(Clone, Copy)]
struct MathState {
    /// 曲线左移的像素数，0–31。
    shift: u16,
    /// 进入页面的时间戳，毫秒。
    entered_at: u32,
}

impl MathState {
    /// 初始状态。
    const fn new() -> Self {
        Self {
            shift: 0,
            entered_at: 0,
        }
    }
}

/// 正弦页的自绘页实现：带坐标轴的曲线，随时间向左移动。
pub(super) struct MathPage {
    /// 帧间状态；回调只拿 `&self`，所以放在共享单元里。
    state: Shared<MathState>,
}

impl MathPage {
    /// 初始状态。
    pub const fn new() -> Self {
        Self {
            state: Shared::new(MathState::new()),
        }
    }
}

impl Default for MathPage {
    fn default() -> Self {
        Self::new()
    }
}

/// 一个周期的正弦值，按 16 归一化，`-16..=16`。
const SINE: [i16; 32] = [
    0, 3, 6, 9, 11, 13, 14, 15, 16, 15, 14, 13, 11, 9, 6, 3, 0, -3, -6, -9, -11, -13, -14, -15,
    -16, -15, -14, -13, -11, -9, -6, -3,
];

impl CustomPageHandler for MathPage {
    fn on_enter(&self, _ui: &mut Runtime, _page: &'static PageRoute) {
        let mut state = MathState::new();
        state.entered_at = APP_NOW.get();
        self.state.set(state);
    }

    fn on_input(&self, ui: &mut Runtime, _page: &'static PageRoute, event: InputEvent) {
        if event.action == InputAction::Ok {
            ui.custom_request_close();
        }
    }

    fn on_tick(&self, _ui: &mut Runtime, _page: &'static PageRoute, now: u32) -> bool {
        let mut state = self.state.get();
        let shift = (now.wrapping_sub(state.entered_at) / 40 % 32) as u16;
        if shift == state.shift {
            return false;
        }
        state.shift = shift;
        self.state.set(state);
        true
    }

    fn on_draw(
        &self,
        ui: &mut Runtime,
        _page: &'static PageRoute,
        x_offset: i16,
        clip_x: i16,
        clip_width: u16,
    ) {
        let state = self.state.get();
        let frame: &mut Frame<'_> = ui.frame_mut();
        let (width, height) = (frame.width(), frame.height());
        frame.set_clip(clip_x, 0, clip_width, height as u16);
        let mid = height / 2;
        let axis_x = 12 + x_offset;
        frame.draw_hline(x_offset, mid, width as u16);
        frame.draw_vline(axis_x, 0, height as u16);
        frame.draw_text(2 + x_offset, 1, "sin");
        let amplitude = mid - 4;
        let origin = axis_x - x_offset;
        let mut previous_y = mid;
        for x in origin..width {
            let index = ((x - origin + state.shift as i16) & 31) as usize;
            let y = mid - SINE[index] * amplitude / 16;
            if x > origin {
                frame.draw_line(x - 1 + x_offset, previous_y, x + x_offset, y);
            }
            previous_y = y;
        }
        frame.reset_clip();
    }
}
