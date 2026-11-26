//! 波形自绘页：相位滚动的折线、进度条与帧率统计。

use kk_ui::{
    CustomPageHandler, DrawMode, Frame, InputAction, InputEvent, PageRoute, Runtime, Shared,
    draw_progress_bar,
};

use super::handlers::{format_u32, update_progress_text};
use super::tables::*;

/// 波形页的帧间状态。
#[derive(Clone, Copy)]
struct WaveState {
    /// 波形相位，0–31。
    phase: u16,
    /// 进度百分比，0–100。
    progress: u16,
    /// 手动模式下进度不再自动推进。
    manual: bool,
    /// 进入页面的时间戳，毫秒。
    entered_at: u32,
    /// 帧率统计窗口起点，毫秒。
    fps_started: u32,
    /// 窗口内实际绘制次数。
    draw_count: u32,
    /// 上次计入绘制次数时看到的应用刷新序号。
    draw_epoch: u32,
}

impl WaveState {
    /// 初始状态。
    const fn new() -> Self {
        Self {
            phase: 0,
            progress: 0,
            manual: false,
            entered_at: 0,
            fps_started: 0,
            draw_count: 0,
            draw_epoch: u32::MAX,
        }
    }
}

/// 波形页的自绘页实现。
pub(super) struct Wave {
    /// 帧间状态；回调只拿 `&self`，所以放在共享单元里。
    state: Shared<WaveState>,
}

impl Wave {
    /// 初始状态。
    pub const fn new() -> Self {
        Self {
            state: Shared::new(WaveState::new()),
        }
    }

    fn reset(&self, now: u32) {
        let mut state = WaveState::new();
        state.entered_at = now;
        state.fps_started = now;
        self.state.set(state);
        FPS_TEXT.set("--");
        update_progress_text(0, false);
    }
}

impl Default for Wave {
    fn default() -> Self {
        Self::new()
    }
}

impl CustomPageHandler for Wave {
    fn on_enter(&self, _ui: &mut Runtime, _page: &'static PageRoute) {
        self.reset(APP_NOW.get());
    }

    fn on_input(&self, ui: &mut Runtime, _page: &'static PageRoute, event: InputEvent) {
        let mut state = self.state.get();
        match event.action {
            InputAction::Ok => {
                ui.custom_request_close();
            }
            InputAction::Up => {
                state.manual = true;
                state.progress = state.progress.saturating_sub(event.steps);
                self.state.set(state);
                update_progress_text(state.progress, true);
                ui.invalidate();
            }
            InputAction::Down => {
                state.manual = true;
                state.progress = (state.progress + event.steps).min(100);
                self.state.set(state);
                update_progress_text(state.progress, true);
                ui.invalidate();
            }
        }
    }

    fn on_tick(&self, _ui: &mut Runtime, _page: &'static PageRoute, now: u32) -> bool {
        let mut state = self.state.get();
        let interval = ANIMATION_INTERVAL.get().max(1) as u32;
        let next = (now.wrapping_sub(state.entered_at) / interval % 32) as u16;
        let mut changed = next != state.phase;
        state.phase = next;
        let elapsed = now.wrapping_sub(state.fps_started);
        if !state.manual {
            let progress = (now.wrapping_sub(state.entered_at) / 50 % 101) as u16;
            if progress != state.progress {
                state.progress = progress;
                update_progress_text(state.progress, state.manual);
                changed = true;
            }
        }
        if elapsed >= 1000 {
            let fps = ((state.draw_count * 1000 + elapsed / 2) / elapsed).min(999);
            let mut buffer = [0u8; 20];
            FPS_TEXT.set(format_u32(fps, "", &mut buffer));
            state.draw_count = 0;
            state.fps_started = now;
            changed = true;
        }
        self.state.set(state);
        changed
    }

    fn on_draw(
        &self,
        ui: &mut Runtime,
        _page: &'static PageRoute,
        x_offset: i16,
        clip_x: i16,
        clip_width: u16,
    ) {
        let mut state = self.state.get();
        if state.draw_epoch != UPDATE_EPOCH.get() {
            state.draw_epoch = UPDATE_EPOCH.get();
            state.draw_count += 1;
            self.state.set(state);
        }
        let frame: &mut Frame<'_> = ui.frame_mut();
        let (width, height) = (frame.width(), frame.height());
        frame.set_clip(clip_x, 0, clip_width, height as u16);
        frame.draw_text(4 + x_offset, 1, "波形");
        frame.draw_text(width - 68 + x_offset, 1, "FPS:");
        FPS_TEXT.with(|text| {
            frame.draw_text(width - 36 + x_offset, 1, text);
        });
        frame.draw_line(width - 11 + x_offset, 5, width - 5 + x_offset, 11);
        frame.draw_line(width - 5 + x_offset, 5, width - 11 + x_offset, 11);
        frame.set_draw_mode(DrawMode::Xor);
        frame.set_draw_mode(DrawMode::Xor);
        frame.draw_rbox(width - 15 + x_offset, 1, 14, 14, 3);
        frame.set_draw_mode(DrawMode::Set);
        let wave: [i16; 16] = [0, -3, -7, -11, -7, -3, 0, 3, 7, 11, 7, 3, 0, -2, -1, 0];
        let base = height / 2;
        let mut previous_y = base;
        for x in 0..width {
            let index = (x as usize + state.phase as usize) & 15;
            let y = base + wave[index] * 8 / 11;
            if x != 0 {
                frame.draw_line(x - 1 + x_offset, previous_y, x + x_offset, y);
            }
            previous_y = y;
        }
        PROGRESS_TEXT.with(|text| {
            frame.draw_text(10 + x_offset, height - 32, text);
        });
        draw_progress_bar(
            frame,
            10 + x_offset,
            height - 16,
            (width - 20) as u16,
            8,
            state.progress,
            100,
        );
        frame.reset_clip();
    }
}
