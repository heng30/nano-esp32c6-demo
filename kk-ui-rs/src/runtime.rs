//! KK_UI 运行时：页面栈、输入消抖、动画调度、事件队列与延迟请求。

use crate::widgets::dialog;
use crate::gfx::{DrawMode, Frame};
use crate::model::*;
use crate::widgets::pages;

/// 导航栈深度上限。
const NAV_DEPTH: usize = 8;
/// 事件队列长度。
const EVENT_QUEUE_LENGTH: usize = 4;
/// 按键消抖时间。
const KEY_DEBOUNCE_MS: u32 = 20;
/// 长按重复的首次延迟。
const KEY_REPEAT_DELAY_MS: u32 = 450;
/// 最小出帧间隔。
const FRAME_INTERVAL_MS: u32 = 20;
/// 首页切换动画时长。
pub(crate) const HOME_MS: u32 = 300;
/// 菜单滚动动画时长。
pub(crate) const MENU_MS: u32 = 200;
/// 页面切换动画时长。
pub(crate) const PAGE_MS: u32 = 220;
/// 信息页滚动动画时长。
pub(crate) const INFO_SCROLL_MS: u32 = 180;
/// 弹窗打开动画时长。
pub(crate) const DIALOG_OPEN_MS: u32 = 180;
/// 弹窗关闭动画时长。
pub(crate) const DIALOG_CLOSE_MS: u32 = 150;
/// 焦点框补间时长。
pub(crate) const FOCUS_MS: u32 = 160;
/// 整数换值动画时长。
pub(crate) const INT_VALUE_MS: u32 = 100;
/// 开关换值动画时长。
pub(crate) const BOOL_VALUE_MS: u32 = 120;
/// Toast 滑入滑出时长。
pub(crate) const TOAST_MS: u32 = 120;
/// Q12 定点数的 1.0。
pub(crate) const Q12_ONE: u16 = 4096;
/// 菜单标题栏高度。
pub(crate) const MENU_HEADER_HEIGHT: i16 = 14;
/// 信息页标题栏高度。
pub(crate) const INFO_HEADER_HEIGHT: i16 = 16;
/// 列表行高。
pub(crate) const ROW_HEIGHT: i16 = 16;
/// 页面切换时旧页面的视差位移。
const PAGE_PARALLAX: i16 = 32;

/// 操作结果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// 成功。
    Ok,
    /// 参数非法。
    InvalidArgument,
    /// 尚未初始化。
    NotInitialized,
    /// 应用描述表配置错误。
    ConfigError,
    /// 当前状态不允许该操作。
    Busy,
    /// 事件队列已满。
    QueueFull,
    /// 导航栈已满。
    NavigationFull,
    /// 当前目标不支持该能力。
    Unsupported,
    /// 当前页面不允许该操作。
    NotAllowed,
}

impl Status {
    /// 稳定短名，便于日志输出。
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::InvalidArgument => "invalid argument",
            Status::NotInitialized => "not initialized",
            Status::ConfigError => "config error",
            Status::Busy => "busy",
            Status::QueueFull => "queue full",
            Status::NavigationFull => "navigation full",
            Status::Unsupported => "unsupported",
            Status::NotAllowed => "not allowed",
        }
    }
}

/// 被记录的错误信息。
#[derive(Clone, Copy)]
pub struct ErrorInfo {
    /// 错误码。
    pub code: Status,
    /// 出错时所在页面。
    pub page: PageId,
    /// 相关条目下标。
    pub index: u16,
}

/// 单页的滚动与焦点状态。
#[derive(Clone, Copy, Default)]
pub(crate) struct PageState {
    /// 页面编号。
    pub page: PageId,
    /// 选中条目下标，等于条目数时表示虚拟返回项。
    pub selected: u16,
    /// 首个可见行的序号。
    pub top: u16,
    /// 滚动量，Q8 定点（1 行 = 16 * 256）。
    pub scroll_q8: i32,
    /// 焦点行相对首个可见行的偏移，Q8 定点。
    pub focus_q8: i32,
}

/// 单个按键的消抖与重复状态。
#[derive(Clone, Copy, Default)]
struct KeyState {
    /// 原始电平最近一次变化的时刻。
    changed_at: u32,
    /// 本次按下生效的时刻。
    pressed_at: u32,
    /// 下一次触发长按重复的时刻。
    repeat_at: u32,
    /// 消抖前的原始电平。
    candidate: bool,
    /// 消抖后的稳定电平。
    stable: bool,
    /// 本次按下是否还未被消费。
    armed: bool,
}

/// 列表滚动与焦点补间的起点。
#[derive(Clone, Copy, Default)]
pub(crate) struct ListAnimation {
    /// 补间起点的滚动量，Q8。
    pub(crate) scroll_from_q8: i32,
    /// 补间起点的焦点偏移，Q8。
    pub(crate) focus_from_q8: i32,
    /// 动画开始的时戳。
    pub(crate) started: u32,
    /// 动画总时长（毫秒）。
    pub(crate) duration: u32,
    /// 是否正在播放。
    pub(crate) active: bool,
}

/// 首页横向补间的起点。
#[derive(Clone, Copy, Default)]
pub(crate) struct HomeAnimation {
    /// 补间起点的图标横向偏移，Q8。
    pub(crate) offset_from_q8: i32,
    /// 补间起点的标签框宽度，Q8。
    pub(crate) label_from_q8: i32,
    /// 补间终点的标签框宽度，Q8。
    pub(crate) label_to_q8: i32,
    /// 动画开始的时戳。
    pub(crate) started: u32,
    /// 是否正在播放。
    pub(crate) active: bool,
}

/// 页面切换动画。
#[derive(Clone, Copy, Default)]
struct PageTransition {
    /// 正在退出页面的状态快照。
    outgoing: PageState,
    /// 动画开始的时戳。
    started: u32,
    /// 是否正在播放。
    active: bool,
    /// 是返回还是进入，决定两页的滑动方向。
    reverse: bool,
}

/// 覆盖层类型。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverlayType {
    /// 无覆盖层。
    None,
    /// 整数编辑器。
    Int,
    /// 开关编辑器。
    Bool,
    /// 确认框。
    Confirm,
    /// 消息框。
    Message,
}

/// 弹窗焦点。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogFocus {
    /// 取值区域。
    Value,
    /// 确定按钮。
    Confirm,
    /// 取消按钮。
    Cancel,
}

/// 覆盖层状态。
#[derive(Clone, Copy)]
pub(crate) struct Overlay {
    /// 当前覆盖层类型。
    pub kind: OverlayType,
    /// 绑定表或确认框表的下标。
    pub reference: u16,
    /// 正文；确认框与消息框使用。
    pub text: &'static str,
    /// 关闭时要入队的事件。
    pub event: EventId,
    /// 编辑器草稿值。
    pub draft: i32,
    /// 打开时的原值，用于判断是否改动。
    pub original: i32,
    /// 上一个草稿值，用于上下列滚动动画。
    pub previous: i32,
    /// 当前焦点矩形，Q8 定点 `(x, y, w, h)`。
    pub focus: (i32, i32, i32, i32),
    /// 补间起点的焦点矩形。
    pub focus_from: (i32, i32, i32, i32),
    /// 焦点位置。
    pub focus_mode: DialogFocus,
    /// 开/关动画开始的时戳。
    pub phase_started: u32,
    /// 焦点补间开始的时戳。
    pub focus_started: u32,
    /// 换值动画开始的时戳。
    pub value_started: u32,
    /// 弹窗出现时对齐的行号。
    pub origin_y: i16,
    /// 开合进度，Q12。
    pub phase_q12: u16,
    /// 关闭动画起点的开合进度，Q12。
    pub phase_from_q12: u16,
    /// 换值动画进度，Q12。
    pub value_q12: u16,
    /// 换值方向，`1` 递增、`-1` 递减。
    pub value_direction: i8,
    /// 是否处于取值编辑态。
    pub editing: bool,
    /// 是否正在播关闭动画。
    pub closing: bool,
    /// 焦点框是否正在补间。
    pub focus_moving: bool,
    /// 取值是否正在换值动画。
    pub value_moving: bool,
}

impl Overlay {
    /// 没有任何覆盖层时的初始状态。
    const EMPTY: Self = Self {
        kind: OverlayType::None,
        reference: 0,
        text: "",
        event: EVENT_NONE,
        draft: 0,
        original: 0,
        previous: 0,
        focus: (0, 0, 0, 0),
        focus_from: (0, 0, 0, 0),
        focus_mode: DialogFocus::Cancel,
        phase_started: 0,
        focus_started: 0,
        value_started: 0,
        origin_y: 0,
        phase_q12: 0,
        phase_from_q12: 0,
        value_q12: Q12_ONE,
        value_direction: 0,
        editing: false,
        closing: false,
        focus_moving: false,
        value_moving: false,
    };
}

/// Toast 状态。
#[derive(Clone, Copy)]
pub(crate) struct Toast {
    /// 提示文本。
    pub text: &'static str,
    /// 滑入动画开始的时戳。
    pub shown_at: u32,
    /// 到期时刻，到达后开始滑出。
    pub expires_at: u32,
    /// 滑入进度，Q12。
    pub phase_q12: u16,
    /// 是否可见。
    pub visible: bool,
    /// 是否正在滑出。
    pub closing: bool,
}

/// 延迟请求类型。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeferredType {
    /// 无请求。
    None,
    /// 自绘页请求关闭。
    Close,
    /// 自绘页结束并上报事件。
    Finish,
    /// 打开整数编辑器。
    Int,
    /// 打开开关编辑器。
    Bool,
    /// 打开确认框。
    Confirm,
    /// 打开消息框。
    Message,
    /// 显示 Toast。
    Toast,
}

/// 一次延迟请求。
#[derive(Clone, Copy)]
pub(crate) struct Deferred {
    /// 请求类型。
    pub kind: DeferredType,
    /// 绑定表或确认框表的下标。
    pub reference: u16,
    /// 消息框或结束时上报的事件。
    pub event: EventId,
    /// 消息框或 Toast 的文本。
    pub text: &'static str,
    /// Toast 时长（毫秒）。
    pub duration: u32,
}

/// Q12 缓动曲线的采样表：`cubic-bezier(0.25, 0, 0.20, 1)` 均匀取样。
const EASE_CURVE: [u16; 65] = [
    0, 6, 23, 53, 98, 159, 236, 329, 439, 565, 704, 853, 1010, 1170, 1331, 1490, 1645, 1795, 1938,
    2075, 2205, 2328, 2444, 2554, 2658, 2756, 2848, 2936, 3018, 3095, 3168, 3237, 3302, 3363, 3420,
    3475, 3526, 3574, 3619, 3661, 3701, 3738, 3773, 3806, 3836, 3864, 3891, 3915, 3938, 3959, 3978,
    3995, 4011, 4025, 4038, 4050, 4060, 4069, 4076, 4082, 4087, 4091, 4094, 4095, 4096,
];

/// 按缓动曲线把已过时间换算成 Q12 进度。
pub(crate) fn ease_q12(elapsed: u32, duration: u32) -> u16 {
    if duration == 0 || elapsed >= duration {
        return Q12_ONE;
    }
    let reference = elapsed * 300 / duration;
    let scaled = reference * 64;
    let index = (scaled / 300) as usize;
    let fraction = scaled % 300;
    let value = EASE_CURVE[index] as u32;
    (value + (EASE_CURVE[index + 1] as u32 - value) * fraction / 300) as u16
}

/// Q12 线性插值。
pub(crate) fn lerp_q12(from: i32, to: i32, progress: u16) -> i32 {
    from + ((to as i64 - from as i64) * progress as i64 / Q12_ONE as i64) as i32
}

/// Q8 定点数四舍五入到整数像素。
pub(crate) fn round_q8(value: i32) -> i16 {
    if value >= 0 {
        ((value + 128) / 256) as i16
    } else {
        ((value - 128) / 256) as i16
    }
}

/// 长按重复的间隔：按已按住时间分三档。
fn repeat_interval(held: u32) -> u32 {
    if held >= 2000 {
        100
    } else if held >= 1000 {
        160
    } else {
        240
    }
}

/// UI 运行时状态；自绘页回调也通过它访问画布与 UI 接口。
pub struct Runtime<'a> {
    /// 应用描述表。
    pub(crate) app: &'static App,
    /// 画布。
    pub(crate) frame: Frame<'a>,
    /// 当前页面状态。
    pub(crate) current: PageState,
    /// 导航栈，栈顶是上一页。
    stack: [PageState; NAV_DEPTH],
    /// 页面切换动画。
    transition: PageTransition,
    /// 首页横向动画。
    pub(crate) home_anim: HomeAnimation,
    /// 列表滚动与焦点动画。
    pub(crate) list_anim: ListAnimation,
    /// 覆盖层状态。
    pub(crate) overlay: Overlay,
    /// Toast 状态。
    pub(crate) toast: Toast,
    /// 待处理的延迟请求。
    deferred: Deferred,
    /// 三个按键的消抖状态，下标对应按键位。
    keys: [KeyState; 3],
    /// 事件环形队列。
    events: [EventId; EVENT_QUEUE_LENGTH],
    /// 队列头部下标。
    event_head: u8,
    /// 队列内事件数。
    event_count: u8,
    /// 最近一次错误。
    error: ErrorInfo,
    /// 最近一次 [`KkUi::update`] 的时戳。
    pub(crate) last_update: u32,
    /// 下一帧最早可出帧的时刻。
    next_frame: u32,
    /// 导航栈内页面数。
    stack_count: u8,
    /// 是否已成功初始化。
    initialized: bool,
    /// 按键状态是否已用首帧输入对齐。
    keys_initialized: bool,
    /// 是否有待重画的内容。
    pub(crate) dirty: bool,
    /// 是否有一帧新画面待上层提交。
    pub(crate) frame_ready: bool,
    /// 是否正在执行自绘页回调。
    pub(crate) in_callback: bool,
    /// [`Runtime::error`] 是否尚未被取走。
    error_valid: bool,
    /// 拒绝操作的边框闪烁计数。
    pub(crate) reject_flash: u8,
    /// 页面切换期间需要吞掉的按键位。
    blocked_keys: u8,
    /// 待通知的自绘页进入/离开事件，由 [`KkUi`] 在安全时机送进回调。
    enter_custom: Option<PageId>,
    leave_custom: Option<PageId>,
}

impl<'a> Runtime<'a> {
    /// 绑定一份应用描述表与帧缓冲。
    pub(crate) fn new(app: &'static App, buffer: &'a mut [u8], width: i16, height: i16) -> Self {
        Self {
            app,
            frame: Frame::new(buffer, width, height),
            current: PageState::default(),
            stack: [PageState::default(); NAV_DEPTH],
            transition: PageTransition::default(),
            home_anim: HomeAnimation::default(),
            list_anim: ListAnimation::default(),
            overlay: Overlay::EMPTY,
            toast: Toast {
                text: "",
                shown_at: 0,
                expires_at: 0,
                phase_q12: 0,
                visible: false,
                closing: false,
            },
            deferred: Deferred {
                kind: DeferredType::None,
                reference: 0,
                event: EVENT_NONE,
                text: "",
                duration: 0,
            },
            keys: [KeyState::default(); 3],
            events: [EVENT_NONE; EVENT_QUEUE_LENGTH],
            event_head: 0,
            event_count: 0,
            error: ErrorInfo {
                code: Status::Ok,
                page: 0,
                index: 0,
            },
            last_update: 0,
            next_frame: 0,
            stack_count: 0,
            initialized: false,
            keys_initialized: false,
            dirty: false,
            frame_ready: false,
            in_callback: false,
            error_valid: false,
            reject_flash: 0,
            blocked_keys: 0,
            enter_custom: None,
            leave_custom: None,
        }
    }

    /// 当前画布。
    pub fn frame(&self) -> &Frame<'_> {
        &self.frame
    }

    /// 当前画布（可变），供自绘页绘制。
    pub fn frame_mut(&mut self) -> &mut Frame<'a> {
        &mut self.frame
    }

    /// 当前页面编号。
    pub fn current_page(&self) -> PageId {
        self.current.page
    }

    /// 页面编号对应的路由。
    pub(crate) fn route(&self, page: PageId) -> Option<PageRoute> {
        if page == PAGE_NONE {
            return None;
        }
        self.app.routes.get(page as usize - 1).copied()
    }

    /// 当前页面类型。
    pub(crate) fn current_page_type(&self) -> PageType {
        self.route(self.current.page)
            .map(|route| route.kind)
            .unwrap_or(PageType::Home)
    }

    /// 记录一条错误，供应用轮询。
    pub(crate) fn record_error(&mut self, code: Status, page: PageId, index: u16) {
        self.error = ErrorInfo { code, page, index };
        self.error_valid = true;
    }

    /// 记录一条配置错误并返回 `ConfigError`。
    fn config_error(&mut self, page: PageId, index: u16) -> Status {
        self.record_error(Status::ConfigError, page, index);
        Status::ConfigError
    }

    /// 检查应用描述表，返回第一处不一致的位置。
    fn validate_app(app: &'static App) -> Result<(), (PageId, u16)> {
        if app.root_page == PAGE_NONE || app.root_page as usize > app.routes.len() {
            return Err((0, 0));
        }
        for (index, route) in app.routes.iter().enumerate() {
            let count = match route.kind {
                PageType::Home => app.home_pages.len(),
                PageType::Menu => app.menu_pages.len(),
                PageType::Info => app.info_pages.len(),
                PageType::Custom => app.custom_page_count as usize,
            };
            if route.index as usize >= count {
                return Err((index as u16 + 1, route.index));
            }
        }
        for (index, page) in app.home_pages.iter().enumerate() {
            for item in page.items {
                if item.target == PAGE_NONE
                    || item.target as usize > app.routes.len()
                    || item.icon.is_empty()
                {
                    return Err((0, index as u16));
                }
            }
        }
        for (index, page) in app.menu_pages.iter().enumerate() {
            if let Some(states) = page.states
                && states.len() < page.items.len().div_ceil(4)
            {
                return Err((0, index as u16));
            }
            for item in page.items {
                let valid = match item.kind {
                    MenuItemType::Page => {
                        item.reference != PAGE_NONE && item.reference as usize <= app.routes.len()
                    }
                    MenuItemType::Action => item.reference != EVENT_NONE,
                    MenuItemType::Int => (item.reference as usize) < app.int_bindings.len(),
                    MenuItemType::Bool => (item.reference as usize) < app.bool_bindings.len(),
                    MenuItemType::Confirm => (item.reference as usize) < app.confirm_descs.len(),
                };
                if !valid {
                    return Err((0, index as u16));
                }
            }
        }
        for (index, binding) in app.int_bindings.iter().enumerate() {
            let value = binding.value.get();
            if binding.minimum > binding.maximum
                || binding.step == 0
                || value < binding.minimum
                || value > binding.maximum
            {
                return Err((0, index as u16));
            }
        }
        Ok(())
    }

    /// 回到根页面并清空导航栈、覆盖层、Toast 与动画，等于「重新开始」。
    pub(crate) fn init(&mut self) -> Status {
        if let Err((page, index)) = Self::validate_app(self.app) {
            return self.config_error(page, index);
        }
        let state = self.reset_page_state(self.app.root_page);
        self.current = state;
        self.stack_count = 0;
        self.transition.active = false;
        self.home_anim.active = false;
        self.list_anim.active = false;
        self.overlay = Overlay::EMPTY;
        self.toast.visible = false;
        self.toast.closing = false;
        self.blocked_keys = 0;
        self.reject_flash = 0;
        self.initialized = true;
        self.dirty = true;
        if self.current_page_type() == PageType::Custom {
            self.enter_custom = Some(self.current.page);
        }
        Status::Ok
    }

    /// 新建一个页面状态，菜单页会先把焦点收缩到合法条目。
    pub(crate) fn reset_page_state(&self, page: PageId) -> PageState {
        let mut state = PageState {
            page,
            ..PageState::default()
        };
        if self.route(page).map(|route| route.kind) == Some(PageType::Menu) {
            pages::menu_repair_focus(self, &mut state);
        }
        state
    }

    /// 事件入队；返回 `false` 表示队列已满。
    pub fn queue_event(&mut self, event: EventId) -> bool {
        if event == EVENT_NONE {
            return true;
        }
        if self.event_count as usize >= EVENT_QUEUE_LENGTH {
            return false;
        }
        let tail = (self.event_head as usize + self.event_count as usize) % EVENT_QUEUE_LENGTH;
        self.events[tail] = event;
        self.event_count += 1;
        true
    }

    /// 取出一个事件。
    pub fn poll_event(&mut self) -> Option<EventId> {
        if self.event_count == 0 {
            return None;
        }
        let event = self.events[self.event_head as usize];
        self.event_head = (self.event_head + 1) % EVENT_QUEUE_LENGTH as u8;
        self.event_count -= 1;
        Some(event)
    }

    /// 取出最近一次错误。
    pub fn poll_error(&mut self) -> Option<ErrorInfo> {
        if !self.error_valid {
            return None;
        }
        self.error_valid = false;
        Some(self.error)
    }

    /// 标记需要重画。
    pub fn invalidate(&mut self) {
        if self.initialized {
            self.dirty = true;
        }
    }

    /// 显示工作是否已经静止：可以安全地做一次阻塞式提交。
    pub fn is_display_idle(&self) -> bool {
        self.initialized && !self.dirty && !self.frame_ready
    }

    /// 当前页面的选中条目下标；菜单页等于条目数时表示虚拟返回项。
    pub fn selected_index(&self) -> u16 {
        self.current.selected
    }

    /// 是否有覆盖层（编辑器、确认框、消息框）正占用输入。
    pub fn dialog_open(&self) -> bool {
        self.overlay.kind != OverlayType::None
    }

    /// 读取整数编辑器草稿；编辑器未打开时返回 `None`。
    pub fn get_int_editor_draft(&self, binding_index: u16) -> Option<i32> {
        if !self.initialized
            || self.overlay.kind != OverlayType::Int
            || self.overlay.closing
            || self.overlay.reference != binding_index
        {
            return None;
        }
        Some(self.overlay.draft)
    }

    /// 设置菜单条目状态。
    pub fn set_menu_item_state(
        &mut self,
        page_id: PageId,
        item_index: u16,
        state: ItemState,
    ) -> Status {
        if !self.initialized {
            return Status::NotInitialized;
        }
        let app = self.app;
        let Some(route) = self.route(page_id) else {
            return Status::InvalidArgument;
        };
        if route.kind != PageType::Menu {
            return Status::InvalidArgument;
        }
        let page = &app.menu_pages[route.index as usize];
        if item_index as usize >= page.items.len() {
            return Status::InvalidArgument;
        }
        let Some(states) = page.states else {
            return Status::Unsupported;
        };
        let shift = (item_index & 3) * 2;
        let mask = 3u8 << shift;
        let slot = &states[(item_index >> 2) as usize];
        slot.set((slot.get() & !mask) | ((state as u8) << shift));
        if self.current.page == page_id {
            self.list_anim.active = false;
            let mut current = self.current;
            pages::menu_repair_focus(self, &mut current);
            self.current = current;
        }
        self.dirty = true;
        Status::Ok
    }

    /// 登记一次延迟请求；已有请求未处理时返回 `Busy`。
    fn defer(
        &mut self,
        kind: DeferredType,
        reference: u16,
        text: &'static str,
        event: EventId,
        duration: u32,
    ) -> Status {
        if self.deferred.kind != DeferredType::None {
            return Status::Busy;
        }
        self.deferred = Deferred {
            kind,
            reference,
            event,
            text,
            duration,
        };
        Status::Ok
    }

    /// 打开整数编辑器。
    pub fn open_int_editor(&mut self, binding_index: u16) -> Status {
        if !self.initialized {
            return Status::NotInitialized;
        }
        if binding_index as usize >= self.app.int_bindings.len() {
            return Status::InvalidArgument;
        }
        if dialog::dialog_active(self) {
            return Status::Busy;
        }
        if self.in_callback {
            return self.defer(DeferredType::Int, binding_index, "", EVENT_NONE, 0);
        }
        let now = self.last_update;
        dialog::open_int(self, binding_index, now)
    }

    /// 打开开关编辑器。
    pub fn open_bool_editor(&mut self, binding_index: u16) -> Status {
        if !self.initialized {
            return Status::NotInitialized;
        }
        if binding_index as usize >= self.app.bool_bindings.len() {
            return Status::InvalidArgument;
        }
        if dialog::dialog_active(self) {
            return Status::Busy;
        }
        if self.in_callback {
            return self.defer(DeferredType::Bool, binding_index, "", EVENT_NONE, 0);
        }
        let now = self.last_update;
        dialog::open_bool(self, binding_index, now)
    }

    /// 打开确认框。
    pub fn open_confirm(&mut self, confirm_index: u16) -> Status {
        if !self.initialized {
            return Status::NotInitialized;
        }
        if confirm_index as usize >= self.app.confirm_descs.len() {
            return Status::InvalidArgument;
        }
        if dialog::dialog_active(self) {
            return Status::Busy;
        }
        if self.in_callback {
            return self.defer(DeferredType::Confirm, confirm_index, "", EVENT_NONE, 0);
        }
        let now = self.last_update;
        dialog::open_confirm(self, confirm_index, now)
    }

    /// 打开消息框。
    pub fn show_message(&mut self, text: &'static str, event: EventId) -> Status {
        if !self.initialized {
            return Status::NotInitialized;
        }
        if dialog::dialog_active(self) {
            return Status::Busy;
        }
        if self.in_callback {
            return self.defer(DeferredType::Message, 0, text, event, 0);
        }
        let now = self.last_update;
        dialog::open_message(self, text, event, now)
    }

    /// 显示 Toast；`duration_ms` 为 0 时使用默认时长。
    pub fn show_toast(&mut self, text: &'static str, duration_ms: u32) -> Status {
        if !self.initialized {
            return Status::NotInitialized;
        }
        if dialog::dialog_active(self) {
            return Status::Busy;
        }
        if self.in_callback {
            return self.defer(DeferredType::Toast, 0, text, EVENT_NONE, duration_ms);
        }
        let now = self.last_update;
        dialog::toast_open(self, text, duration_ms, now)
    }

    /// 自绘页请求关闭自己。
    pub fn custom_request_close(&mut self) -> Status {
        if !self.initialized {
            return Status::NotInitialized;
        }
        if self.current_page_type() != PageType::Custom {
            return Status::NotAllowed;
        }
        if self.transition.active || dialog::dialog_active(self) {
            return Status::Busy;
        }
        if self.stack_count == 0 {
            return Status::NotAllowed;
        }
        if self.in_callback {
            return self.defer(DeferredType::Close, 0, "", EVENT_NONE, 0);
        }
        let now = self.last_update;
        self.navigate_back(now)
    }

    /// 自绘页结束时上报事件并返回上一页。
    pub fn custom_finish(&mut self, event: EventId) -> Status {
        if !self.initialized {
            return Status::NotInitialized;
        }
        if event == EVENT_NONE {
            return Status::InvalidArgument;
        }
        if self.current_page_type() != PageType::Custom {
            return Status::NotAllowed;
        }
        if self.transition.active || dialog::dialog_active(self) {
            return Status::Busy;
        }
        if self.stack_count == 0 {
            return Status::NotAllowed;
        }
        if self.in_callback {
            return self.defer(DeferredType::Finish, 0, "", event, 0);
        }
        if !self.queue_event(event) {
            return Status::QueueFull;
        }
        let now = self.last_update;
        self.navigate_back(now)
    }

    /// 启动页面切换动画，并吞掉切换期间仍按住的按键。
    fn start_transition(&mut self, outgoing: PageState, reverse: bool, now: u32) {
        let mut stable = 0u8;
        for (index, key) in self.keys.iter().enumerate() {
            if key.stable {
                stable |= 1 << index;
            }
        }
        self.blocked_keys |= stable;
        let leave_custom =
            self.route(outgoing.page).map(|route| route.kind) == Some(PageType::Custom);
        self.transition = PageTransition {
            outgoing,
            started: now,
            active: true,
            reverse,
        };
        if leave_custom {
            self.leave_custom = Some(outgoing.page);
        }
        self.dirty = true;
    }

    /// 进入页面。
    pub fn navigate_to(&mut self, page: PageId, now: u32) -> Status {
        let Some(route) = self.route(page) else {
            return Status::InvalidArgument;
        };
        if self.transition.active {
            return Status::Busy;
        }
        if self.stack_count as usize >= NAV_DEPTH {
            self.record_error(Status::NavigationFull, self.current.page, page);
            self.reject_flash = 2;
            self.dirty = true;
            return Status::NavigationFull;
        }
        if self.current_page_type() == PageType::Custom && route.kind == PageType::Custom {
            return Status::NotAllowed;
        }
        let outgoing = self.current;
        self.stack[self.stack_count as usize] = outgoing;
        self.stack_count += 1;
        let state = self.reset_page_state(page);
        self.current = state;
        self.home_anim.active = false;
        self.list_anim.active = false;
        self.start_transition(outgoing, false, now);
        if route.kind == PageType::Custom {
            self.enter_custom = Some(page);
        }
        Status::Ok
    }

    /// 返回上一页。
    pub fn navigate_back(&mut self, now: u32) -> Status {
        if self.transition.active {
            return Status::Busy;
        }
        if self.stack_count == 0 {
            return Status::NotAllowed;
        }
        let outgoing = self.current;
        self.stack_count -= 1;
        self.current = self.stack[self.stack_count as usize];
        if self.current_page_type() == PageType::Menu {
            let mut current = self.current;
            pages::menu_repair_focus(self, &mut current);
            self.current = current;
        }
        self.home_anim.active = false;
        self.list_anim.active = false;
        self.start_transition(outgoing, true, now);
        if self.current_page_type() == PageType::Custom {
            self.enter_custom = Some(self.current.page);
        }
        Status::Ok
    }

    /// 页面切换动画推进。
    pub(crate) fn animate_navigation(&mut self, now: u32) {
        if !self.transition.active {
            return;
        }
        self.dirty = true;
        if now.wrapping_sub(self.transition.started) >= PAGE_MS {
            self.transition.active = false;
        }
    }

    /// 处理延迟请求。
    pub(crate) fn apply_deferred(&mut self, now: u32) {
        let request = self.deferred;
        self.deferred.kind = DeferredType::None;
        match request.kind {
            DeferredType::None => {}
            DeferredType::Close => {
                if self.current_page_type() == PageType::Custom
                    && self.stack_count != 0
                    && !self.transition.active
                    && !dialog::dialog_active(self)
                {
                    self.navigate_back(now);
                }
            }
            DeferredType::Finish => {
                if self.current_page_type() != PageType::Custom
                    || self.stack_count == 0
                    || self.transition.active
                    || dialog::dialog_active(self)
                {
                    self.record_error(Status::NotAllowed, self.current.page, request.event);
                } else if self.queue_event(request.event) {
                    self.navigate_back(now);
                } else {
                    self.record_error(Status::QueueFull, self.current.page, request.event);
                    self.reject_flash = 2;
                    self.dirty = true;
                }
            }
            DeferredType::Int => {
                dialog::open_int(self, request.reference, now);
            }
            DeferredType::Bool => {
                dialog::open_bool(self, request.reference, now);
            }
            DeferredType::Confirm => {
                dialog::open_confirm(self, request.reference, now);
            }
            DeferredType::Message => {
                dialog::open_message(self, request.text, request.event, now);
            }
            DeferredType::Toast => {
                dialog::toast_open(self, request.text, request.duration, now);
            }
        }
    }
}

/// 自绘页适配器：应用实现它来接管 `PageType::Custom` 页面。
pub trait CustomPage {
    /// 进入页面。
    fn on_enter(&mut self, ui: &mut Runtime<'_>, page: PageId) {
        let _ = (ui, page);
    }

    /// 离开页面；在切换动画结束后调用。
    fn on_leave(&mut self, ui: &mut Runtime<'_>, page: PageId) {
        let _ = (ui, page);
    }

    /// 处理输入。
    fn on_input(&mut self, ui: &mut Runtime<'_>, page: PageId, event: InputEvent) {
        let _ = (ui, page, event);
    }

    /// 逐帧推进；返回 `true` 表示需要重画。
    fn on_tick(&mut self, ui: &mut Runtime<'_>, page: PageId, now: u32) -> bool {
        let _ = (ui, page, now);
        false
    }

    /// 绘制页面；`x_offset` 为视差位移，`clip_x`/`clip_width` 是可见区域。
    fn on_draw(
        &mut self,
        ui: &mut Runtime<'_>,
        page: PageId,
        x_offset: i16,
        clip_x: i16,
        clip_width: u16,
    ) {
        let _ = (ui, page, x_offset, clip_x, clip_width);
    }
}

/// UI 本体：运行时 + 自绘页适配器。
pub struct KkUi<'a, C: CustomPage> {
    /// 运行时状态。
    rt: Runtime<'a>,
    /// 应用提供的自绘页实现。
    custom: C,
}

impl<'a, C: CustomPage> KkUi<'a, C> {
    /// 绑定应用描述表、自绘页实现与一块 `width * ceil(height / 8)` 字节的帧缓冲。
    pub fn new(
        app: &'static App,
        custom: C,
        buffer: &'a mut [u8],
        width: i16,
        height: i16,
    ) -> Self {
        Self {
            rt: Runtime::new(app, buffer, width, height),
            custom,
        }
    }

    /// 校验配置，回到根页面并清空导航栈、覆盖层与动画。
    pub fn init(&mut self) -> Status {
        let status = self.rt.init();
        self.flush_custom();
        status
    }

    /// 推进一帧：动画、输入、延迟请求、重画。
    pub fn update(&mut self, now: u32, input: Input) -> Status {
        if !self.rt.initialized {
            return Status::NotInitialized;
        }
        if !self.rt.keys_initialized {
            self.rt.next_frame = now;
        }
        self.rt.last_update = now;
        self.rt.animate_navigation(now);
        pages::home_animate(&mut self.rt, now);
        pages::menu_animate(&mut self.rt, now);
        pages::info_animate(&mut self.rt, now);
        dialog::animate(&mut self.rt, now);
        self.flush_custom();
        self.process_input(now, input);
        self.rt.apply_deferred(now);
        self.flush_custom();
        self.custom_tick(now);
        self.rt.apply_deferred(now);
        self.flush_custom();

        if self.rt.dirty && (now.wrapping_sub(self.rt.next_frame) as i32) >= 0 {
            let missed = now.wrapping_sub(self.rt.next_frame) / FRAME_INTERVAL_MS;
            self.rt.next_frame = self
                .rt
                .next_frame
                .wrapping_add((missed + 1).wrapping_mul(FRAME_INTERVAL_MS));
            self.rt.dirty = false;
            self.draw_scene();
            self.rt.frame_ready = true;
        }
        Status::Ok
    }

    /// 当前画布。
    pub fn frame(&self) -> &Frame<'_> {
        self.rt.frame()
    }

    /// 当前页面编号。
    pub fn current_page(&self) -> PageId {
        self.rt.current_page()
    }

    /// 取走「有新帧待提交」的标记。
    pub fn take_frame_ready(&mut self) -> bool {
        let ready = self.rt.frame_ready;
        self.rt.frame_ready = false;
        ready
    }

    /// 取走一个应用事件。
    pub fn poll_event(&mut self) -> Option<EventId> {
        self.rt.poll_event()
    }

    /// 取走最近一次错误。
    pub fn poll_error(&mut self) -> Option<ErrorInfo> {
        self.rt.poll_error()
    }

    /// 设置菜单条目状态。
    pub fn set_menu_item_state(
        &mut self,
        page_id: PageId,
        item_index: u16,
        state: ItemState,
    ) -> Status {
        self.rt.set_menu_item_state(page_id, item_index, state)
    }

    /// 读取整数编辑器草稿。
    pub fn get_int_editor_draft(&self, binding_index: u16) -> Option<i32> {
        self.rt.get_int_editor_draft(binding_index)
    }

    /// 显示工作是否已经静止。
    pub fn is_display_idle(&self) -> bool {
        self.rt.is_display_idle()
    }

    /// 当前页面的选中条目下标；菜单页等于条目数时表示虚拟返回项。
    pub fn selected_index(&self) -> u16 {
        self.rt.selected_index()
    }

    /// 是否有覆盖层（编辑器、确认框、消息框）正占用输入。
    pub fn dialog_open(&self) -> bool {
        self.rt.dialog_open()
    }

    /// 标记需要重画。
    pub fn invalidate(&mut self) {
        self.rt.invalidate();
    }

    /// 打开整数编辑器。
    pub fn open_int_editor(&mut self, binding_index: u16) -> Status {
        self.rt.open_int_editor(binding_index)
    }

    /// 打开开关编辑器。
    pub fn open_bool_editor(&mut self, binding_index: u16) -> Status {
        self.rt.open_bool_editor(binding_index)
    }

    /// 打开确认框。
    pub fn open_confirm(&mut self, confirm_index: u16) -> Status {
        self.rt.open_confirm(confirm_index)
    }

    /// 打开消息框。
    pub fn show_message(&mut self, text: &'static str, event: EventId) -> Status {
        self.rt.show_message(text, event)
    }

    /// 显示 Toast。
    pub fn show_toast(&mut self, text: &'static str, duration_ms: u32) -> Status {
        self.rt.show_toast(text, duration_ms)
    }

    /// 把待发的进入/离开事件送进自绘页回调。
    fn flush_custom(&mut self) {
        while let Some(page) = self.rt.enter_custom.take() {
            let Self { rt, custom } = self;
            rt.in_callback = true;
            custom.on_enter(rt, page);
            rt.in_callback = false;
        }
        while let Some(page) = self.rt.leave_custom.take() {
            let Self { rt, custom } = self;
            rt.in_callback = true;
            custom.on_leave(rt, page);
            rt.in_callback = false;
        }
    }

    /// 逐帧推进当前自绘页；回调返回 `true` 时要求重画。
    fn custom_tick(&mut self, now: u32) {
        if self.rt.current_page_type() != PageType::Custom || self.rt.transition.active {
            return;
        }
        let Self { rt, custom } = self;
        let page = rt.current.page;
        rt.in_callback = true;
        let redraw = custom.on_tick(rt, page, now);
        rt.in_callback = false;
        if redraw {
            rt.dirty = true;
        }
    }

    /// 消抖、识别长按重复与编码器，并发出一条归一化输入事件。
    #[allow(clippy::needless_range_loop)]
    fn process_input(&mut self, now: u32, input: Input) {
        let mut press = 0u8;
        let mut repeat = 0u8;
        let keys = input.keys & (KEY_UP | KEY_DOWN | KEY_OK);
        if !self.rt.keys_initialized {
            for index in 0..3 {
                let down = keys & (1 << index) != 0;
                let key = &mut self.rt.keys[index];
                key.changed_at = now;
                key.pressed_at = now;
                key.repeat_at = now;
                key.candidate = down;
                key.stable = down;
                key.armed = !down;
            }
            self.rt.keys_initialized = true;
        } else {
            for index in 0..3 {
                let key = &mut self.rt.keys[index];
                let down = keys & (1 << index) != 0;
                if down != key.candidate {
                    key.candidate = down;
                    key.changed_at = now;
                }
                if key.candidate != key.stable
                    && now.wrapping_sub(key.changed_at) >= KEY_DEBOUNCE_MS
                {
                    key.stable = key.candidate;
                    if !key.stable {
                        key.armed = true;
                    } else if key.armed {
                        key.pressed_at = now;
                        key.repeat_at = now.wrapping_add(KEY_REPEAT_DELAY_MS);
                        press |= 1 << index;
                    }
                }
                // 只有上下键支持长按重复。
                if index < 2
                    && key.stable
                    && key.candidate
                    && key.armed
                    && (now.wrapping_sub(key.repeat_at) as i32) >= 0
                {
                    repeat |= 1 << index;
                    key.repeat_at =
                        now.wrapping_add(repeat_interval(now.wrapping_sub(key.pressed_at)));
                }
            }
        }

        let mut stable = 0u8;
        for (index, key) in self.rt.keys.iter().enumerate() {
            if key.stable {
                stable |= 1 << index;
            }
        }
        self.rt.blocked_keys &= stable;
        if self.rt.transition.active {
            self.rt.blocked_keys |= stable;
            return;
        }
        press &= !self.rt.blocked_keys;
        repeat &= !self.rt.blocked_keys;

        let mut event = InputEvent {
            action: InputAction::Up,
            source: InputSource::Press,
            steps: 1,
        };
        if press & KEY_OK != 0 {
            event.action = InputAction::Ok;
            self.dispatch_input(event, now);
            return;
        }
        if input.encoder_delta != 0 {
            let delta = input.encoder_delta as i32;
            event.action = if delta < 0 {
                InputAction::Up
            } else {
                InputAction::Down
            };
            event.source = InputSource::Encoder;
            event.steps = delta.unsigned_abs().min(u16::MAX as u32) as u16;
            self.dispatch_input(event, now);
            return;
        }
        if stable & (KEY_UP | KEY_DOWN) == (KEY_UP | KEY_DOWN) {
            return;
        }
        if (press | repeat) & KEY_UP != 0 {
            event.action = InputAction::Up;
            event.source = if repeat & KEY_UP != 0 {
                InputSource::Repeat
            } else {
                InputSource::Press
            };
            self.dispatch_input(event, now);
        } else if (press | repeat) & KEY_DOWN != 0 {
            event.action = InputAction::Down;
            event.source = if repeat & KEY_DOWN != 0 {
                InputSource::Repeat
            } else {
                InputSource::Press
            };
            self.dispatch_input(event, now);
        }
    }

    /// 按当前页面类型把输入分发下去；有覆盖层时优先给弹窗。
    fn dispatch_input(&mut self, event: InputEvent, now: u32) {
        if dialog::dialog_active(&self.rt) {
            dialog::input(&mut self.rt, event, now);
            return;
        }
        match self.rt.current_page_type() {
            PageType::Home => pages::home_input(&mut self.rt, event, now),
            PageType::Menu => pages::menu_input(&mut self.rt, event, now),
            PageType::Info => pages::info_input(&mut self.rt, event, now),
            PageType::Custom => {
                let Self { rt, custom } = self;
                let page = rt.current.page;
                rt.in_callback = true;
                custom.on_input(rt, page, event);
                rt.in_callback = false;
            }
        }
    }

    /// 按页面类型绘制一页；`x_offset` 为视差位移，`clip_x`/`clip_width` 是可见区域。
    fn draw_page(
        &mut self,
        state: PageState,
        is_current: bool,
        x_offset: i16,
        clip_x: i16,
        clip_width: u16,
    ) {
        let Some(route) = self.rt.route(state.page) else {
            return;
        };
        match route.kind {
            PageType::Home => pages::home_draw(
                &mut self.rt,
                state,
                is_current,
                x_offset,
                clip_x,
                clip_width,
            ),
            PageType::Menu => pages::menu_draw(
                &mut self.rt,
                state,
                is_current,
                x_offset,
                clip_x,
                clip_width,
            ),
            PageType::Info => pages::info_draw(
                &mut self.rt,
                state,
                is_current,
                x_offset,
                clip_x,
                clip_width,
            ),
            PageType::Custom => {
                let Self { rt, custom } = self;
                rt.in_callback = true;
                custom.on_draw(rt, state.page, x_offset, clip_x, clip_width);
                rt.in_callback = false;
            }
        }
    }

    /// 清屏并重画整个场景：页面、切换动画、拒绝闪烁、弹窗与 Toast。
    fn draw_scene(&mut self) {
        self.rt.frame.clear();
        self.rt.frame.reset_clip();
        self.rt.frame.set_draw_mode(DrawMode::Set);
        let now = self.rt.last_update;
        let width = self.rt.frame.width();
        let height = self.rt.frame.height();

        if self.rt.transition.active {
            let progress = ease_q12(now.wrapping_sub(self.rt.transition.started), PAGE_MS);
            let outgoing = self.rt.transition.outgoing;
            let current = self.rt.current;
            let boundary;
            if !self.rt.transition.reverse {
                let outgoing_x =
                    -((PAGE_PARALLAX as i32 * progress as i32) / Q12_ONE as i32) as i16;
                let current_x =
                    (width as i32 * (Q12_ONE as i32 - progress as i32) / Q12_ONE as i32) as i16;
                boundary = current_x;
                if boundary > 0 {
                    self.draw_page(outgoing, false, outgoing_x, 0, boundary as u16);
                }
                if boundary < width {
                    self.draw_page(
                        current,
                        true,
                        current_x,
                        boundary,
                        (width - boundary) as u16,
                    );
                }
            } else {
                let outgoing_x = (PAGE_PARALLAX as i32 * progress as i32 / Q12_ONE as i32) as i16;
                boundary = (width as i32 * progress as i32 / Q12_ONE as i32) as i16;
                let current_x = boundary - width;
                if boundary > 0 {
                    self.draw_page(current, true, current_x, 0, boundary as u16);
                }
                if boundary < width {
                    self.draw_page(
                        outgoing,
                        false,
                        outgoing_x,
                        boundary,
                        (width - boundary) as u16,
                    );
                }
            }
            if (0..width).contains(&boundary) {
                self.rt.frame.reset_clip();
                self.rt.frame.draw_vline(boundary, 0, height as u16);
            }
        } else {
            self.draw_page(self.rt.current, true, 0, 0, width as u16);
        }

        if self.rt.reject_flash > 1 {
            self.rt.frame.set_draw_mode(DrawMode::Xor);
            self.rt
                .frame
                .draw_rframe(1, 1, (width - 2) as u16, (height - 2) as u16, 3);
            self.rt.frame.set_draw_mode(DrawMode::Set);
            self.rt.reject_flash = 1;
            self.rt.dirty = true;
        } else if self.rt.reject_flash != 0 {
            self.rt.reject_flash = 0;
        }
        dialog::draw(&mut self.rt);
        dialog::draw_toast(&mut self.rt);
        self.rt.frame.reset_clip();
        self.rt.frame.set_draw_mode(DrawMode::Set);
    }
}
