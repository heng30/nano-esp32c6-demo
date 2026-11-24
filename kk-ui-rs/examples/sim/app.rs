//! 移植自 KK_UI 标准 Demo 的应用层：页面表、业务状态与波形自绘页。
//!
//! 本文件只依赖 `kk-ui-rs` 与 `core`，可以在主机上原样编入渲染测试工程。
//! 与 `ili9341-lcd-kkui/src/app.rs` 是同一份内容（字体图标在 `assets/` 各一份），
//! 改了一边记得同步另一边。

use kk_ui::{
    App, BoolBinding, ConfirmDesc, CustomPage, DrawMode, EventId, Font, Fonts, Frame, HomeItem,
    HomePage, InfoPage, InfoRow, InputAction, InputEvent, IntBinding, ItemState, KEY_DOWN, KEY_OK,
    KkUi, MenuItem, MenuItemType, MenuPage, PageId, PageRoute, PageType, Runtime, Shared, Text,
    Texts, draw_progress_bar,
};

/// 页面：首页。
pub const PAGE_HOME: PageId = 1;
/// 页面：设置。
pub const PAGE_SETTINGS: PageId = 2;
/// 页面：数据监测。
pub const PAGE_MONITOR: PageId = 3;
/// 页面：波形。
pub const PAGE_WAVE: PageId = 4;
/// 页面：关于设备。
pub const PAGE_ABOUT: PageId = 5;
/// 页面：组件演示。
pub const PAGE_COMPONENTS: PageId = 6;

/// 绑定下标：屏幕亮度。
const INT_BRIGHTNESS: u16 = 0;
/// 绑定下标：动画间隔。
const INT_INTERVAL: u16 = 1;

/// 绑定下标：开关。
const BOOL_SWITCH: u16 = 0;
/// 绑定下标：显示条目。
const BOOL_SHOW_ITEM: u16 = 1;
/// 绑定下标：禁用条目。
const BOOL_DISABLE_ITEM: u16 = 2;

/// 确认框下标：恢复默认设置。
const CONFIRM_RESET: u16 = 0;

/// 事件：亮度改变。
pub const EVENT_BRIGHTNESS_CHANGED: EventId = 1;
/// 事件：开关改变。
pub const EVENT_SWITCH_CHANGED: EventId = 2;
/// 事件：动画间隔改变。
pub const EVENT_INTERVAL_CHANGED: EventId = 3;
/// 事件：确认恢复默认。
pub const EVENT_RESET_CONFIRMED: EventId = 4;
/// 事件：取消恢复默认。
pub const EVENT_RESET_CANCELLED: EventId = 5;
/// 事件：弹出消息框。
pub const EVENT_SHOW_MESSAGE: EventId = 6;
/// 事件：消息框已确认。
pub const EVENT_MESSAGE_ACKNOWLEDGED: EventId = 7;
/// 事件：条目可见性改变。
pub const EVENT_ADVANCED_VISIBILITY_CHANGED: EventId = 8;
/// 事件：条目禁用改变。
pub const EVENT_DEMO_LOCK_CHANGED: EventId = 9;
/// 事件：普通操作示例。
pub const EVENT_ADVANCED_ACTION: EventId = 10;
/// 事件：弹出 Toast。
pub const EVENT_SHOW_TOAST: EventId = 11;

/// 首页标签字体。
pub static FONT_HOME: Font = Font::new(include_bytes!("assets/font_home.bin"));
/// 标题字体。
pub static FONT_TITLE: Font = Font::new(include_bytes!("assets/font_title.bin"));
/// 正文字体。
pub static FONT_BODY: Font = Font::new(include_bytes!("assets/font_body.bin"));

// 图标是 32×32 XBM，每个 128 字节。
static ICON_SETTINGS: &[u8] = include_bytes!("assets/icon0.bin");
static ICON_MONITOR: &[u8] = include_bytes!("assets/icon1.bin");
static ICON_WAVE: &[u8] = include_bytes!("assets/icon2.bin");
static ICON_ABOUT: &[u8] = include_bytes!("assets/icon3.bin");

static BRIGHTNESS: Shared<i32> = Shared::new(80);
static ANIMATION_INTERVAL: Shared<i32> = Shared::new(20);
static DEMO_SWITCH: Shared<bool> = Shared::new(true);
static SHOW_ITEM: Shared<bool> = Shared::new(true);
static DISABLE_ITEM: Shared<bool> = Shared::new(false);

/// 设置页条目状态（压缩成 2 bit 一项）。
static SETTINGS_STATES: [Shared<u8>; 2] = [Shared::new(0), Shared::new(0)];
/// 组件演示页条目状态。
static COMPONENT_STATES: [Shared<u8>; 2] = [Shared::new(0), Shared::new(0)];

static UPTIME: Text = Text::literal("0s");
static BRIGHTNESS_TEXT: Text = Text::literal("80%");
static SWITCH_TEXT: Text = Text::literal("开");
static INTERVAL_TEXT: Text = Text::literal("20ms");
static BRIGHTNESS_STATE: Text = Text::literal("等待");
static SCREEN_INFO: Text = Text::literal("128x64@2x");
static DRIVER_INFO: Text = Text::literal("kk-ui-rs");
static VERSION_INFO: Text = Text::literal("0.1.0");
static DEVICE_INFO: Text = Text::literal("KK_UI");
static REFRESH_INFO: Text = Text::literal("Blocking");
static YES_INFO: Text = Text::literal("否");
static DPAD_INFO: Text = Text::literal("选择/调整");
static OK_INFO: Text = Text::literal("进入/确认");
static EXIT_INFO: Text = Text::literal("确定退出");
static FPS_INFO: Text = Text::literal("波形绘制");
static FPS_TEXT: Text = Text::literal("--");
static PROGRESS_TEXT: Text = Text::literal("自动 0%");

static APP_STARTED_AT: Shared<u32> = Shared::new(0);
static INFO_UPDATED_AT: Shared<u32> = Shared::new(0);
static APP_NOW: Shared<u32> = Shared::new(0);
static UPDATE_EPOCH: Shared<u32> = Shared::new(0);
/// 待补发的 Toast：覆盖层占用时先记下，下一帧再试。
static PENDING_TOAST: Shared<Option<(&'static str, u32)>> = Shared::new(None);
/// 亮度控制只上报一次「不支持」。
static BRIGHTNESS_REPORTED: Shared<bool> = Shared::new(false);

static HOME_ITEMS: [HomeItem; 4] = [
    HomeItem {
        label: "设置",
        icon: ICON_SETTINGS,
        target: PAGE_SETTINGS,
    },
    HomeItem {
        label: "数据监测",
        icon: ICON_MONITOR,
        target: PAGE_MONITOR,
    },
    HomeItem {
        label: "波形",
        icon: ICON_WAVE,
        target: PAGE_WAVE,
    },
    HomeItem {
        label: "关于设备",
        icon: ICON_ABOUT,
        target: PAGE_ABOUT,
    },
];

static HOME_PAGES: [HomePage; 1] = [HomePage { items: &HOME_ITEMS }];

static SETTINGS_ITEMS: [MenuItem; 6] = [
    MenuItem {
        label: "屏幕亮度",
        kind: MenuItemType::Int,
        reference: INT_BRIGHTNESS,
    },
    MenuItem {
        label: "开关",
        kind: MenuItemType::Bool,
        reference: BOOL_SWITCH,
    },
    MenuItem {
        label: "组件演示",
        kind: MenuItemType::Page,
        reference: PAGE_COMPONENTS,
    },
    MenuItem {
        label: "确认框",
        kind: MenuItemType::Confirm,
        reference: CONFIRM_RESET,
    },
    MenuItem {
        label: "消息框",
        kind: MenuItemType::Action,
        reference: EVENT_SHOW_MESSAGE,
    },
    MenuItem {
        label: "Toast",
        kind: MenuItemType::Action,
        reference: EVENT_SHOW_TOAST,
    },
];

static COMPONENT_ITEMS: [MenuItem; 4] = [
    MenuItem {
        label: "动画间隔",
        kind: MenuItemType::Int,
        reference: INT_INTERVAL,
    },
    MenuItem {
        label: "显示条目",
        kind: MenuItemType::Bool,
        reference: BOOL_SHOW_ITEM,
    },
    MenuItem {
        label: "禁用条目",
        kind: MenuItemType::Bool,
        reference: BOOL_DISABLE_ITEM,
    },
    MenuItem {
        label: "操作示例",
        kind: MenuItemType::Action,
        reference: EVENT_ADVANCED_ACTION,
    },
];

static MENU_PAGES: [MenuPage; 2] = [
    MenuPage {
        title: "设置",
        items: &SETTINGS_ITEMS,
        states: Some(&SETTINGS_STATES),
    },
    MenuPage {
        title: "组件演示",
        items: &COMPONENT_ITEMS,
        states: Some(&COMPONENT_STATES),
    },
];

static MONITOR_ROWS: [InfoRow; 5] = [
    InfoRow {
        name: "运行时间",
        value: &UPTIME,
    },
    InfoRow {
        name: "屏幕亮度",
        value: &BRIGHTNESS_TEXT,
    },
    InfoRow {
        name: "开关状态",
        value: &SWITCH_TEXT,
    },
    InfoRow {
        name: "动画间隔",
        value: &INTERVAL_TEXT,
    },
    InfoRow {
        name: "亮度控制",
        value: &BRIGHTNESS_STATE,
    },
];

static ABOUT_ROWS: [InfoRow; 10] = [
    InfoRow {
        name: "设备名称",
        value: &DEVICE_INFO,
    },
    InfoRow {
        name: "UI版本",
        value: &VERSION_INFO,
    },
    InfoRow {
        name: "显示驱动",
        value: &DRIVER_INFO,
    },
    InfoRow {
        name: "屏幕尺寸",
        value: &SCREEN_INFO,
    },
    InfoRow {
        name: "刷新方式",
        value: &REFRESH_INFO,
    },
    InfoRow {
        name: "断电保留",
        value: &YES_INFO,
    },
    InfoRow {
        name: "方向键",
        value: &DPAD_INFO,
    },
    InfoRow {
        name: "确定键",
        value: &OK_INFO,
    },
    InfoRow {
        name: "信息波形",
        value: &EXIT_INFO,
    },
    InfoRow {
        name: "帧率口径",
        value: &FPS_INFO,
    },
];

static INFO_PAGES: [InfoPage; 2] = [
    InfoPage {
        title: "数据监测",
        rows: &MONITOR_ROWS,
    },
    InfoPage {
        title: "关于设备",
        rows: &ABOUT_ROWS,
    },
];

static INT_BINDINGS: [IntBinding; 2] = [
    IntBinding {
        title: "屏幕亮度",
        value: &BRIGHTNESS,
        minimum: 10,
        maximum: 100,
        step: 1,
        unit: "%",
        changed_event: EVENT_BRIGHTNESS_CHANGED,
    },
    IntBinding {
        title: "动画间隔",
        value: &ANIMATION_INTERVAL,
        minimum: 10,
        maximum: 100,
        step: 10,
        unit: "ms",
        changed_event: EVENT_INTERVAL_CHANGED,
    },
];

static BOOL_BINDINGS: [BoolBinding; 3] = [
    BoolBinding {
        title: "开关",
        value: &DEMO_SWITCH,
        changed_event: EVENT_SWITCH_CHANGED,
    },
    BoolBinding {
        title: "显示条目",
        value: &SHOW_ITEM,
        changed_event: EVENT_ADVANCED_VISIBILITY_CHANGED,
    },
    BoolBinding {
        title: "禁用条目",
        value: &DISABLE_ITEM,
        changed_event: EVENT_DEMO_LOCK_CHANGED,
    },
];

static CONFIRM_DESCS: [ConfirmDesc; 1] = [ConfirmDesc {
    text: "恢复默认设置？\n包含屏幕亮度",
    confirmed_event: EVENT_RESET_CONFIRMED,
    cancelled_event: EVENT_RESET_CANCELLED,
}];

static ROUTES: [PageRoute; 6] = [
    PageRoute {
        kind: PageType::Home,
        index: 0,
    },
    PageRoute {
        kind: PageType::Menu,
        index: 0,
    },
    PageRoute {
        kind: PageType::Info,
        index: 0,
    },
    PageRoute {
        kind: PageType::Custom,
        index: 0,
    },
    PageRoute {
        kind: PageType::Info,
        index: 1,
    },
    PageRoute {
        kind: PageType::Menu,
        index: 1,
    },
];

/// 整个应用描述表。
pub static APP: App = App {
    root_page: PAGE_HOME,
    routes: &ROUTES,
    home_pages: &HOME_PAGES,
    menu_pages: &MENU_PAGES,
    info_pages: &INFO_PAGES,
    custom_page_count: 1,
    int_bindings: &INT_BINDINGS,
    bool_bindings: &BOOL_BINDINGS,
    confirm_descs: &CONFIRM_DESCS,
    fonts: Fonts {
        home: &FONT_HOME,
        title: &FONT_TITLE,
        body: &FONT_BODY,
    },
    texts: Texts {
        return_text: "返回",
        cancel_text: "取消",
        confirm_text: "确定",
        on_text: "开",
        off_text: "关",
        message_title: "这是消息框",
    },
};

/// 波形页的帧间状态。
pub struct Wave {
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

impl Wave {
    /// 初始状态。
    pub const fn new() -> Self {
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

    fn reset(&mut self, now: u32) {
        self.phase = 0;
        self.progress = 0;
        self.manual = false;
        self.entered_at = now;
        self.fps_started = now;
        self.draw_count = 0;
        self.draw_epoch = u32::MAX;
        FPS_TEXT.set("--");
        update_progress_text(self.progress, self.manual);
    }
}

impl Default for Wave {
    fn default() -> Self {
        Self::new()
    }
}

fn update_progress_text(progress: u16, manual: bool) {
    let mut buffer = [0u8; 20];
    let prefix = if manual { "手动 " } else { "自动 " };
    let mut text = [0u8; 20];
    let prefix_len = prefix.len();
    text[..prefix_len].copy_from_slice(prefix.as_bytes());
    let digits = format_u32(progress as u32, "%", &mut buffer);
    let len = prefix_len + digits.len();
    text[prefix_len..len].copy_from_slice(digits.as_bytes());
    PROGRESS_TEXT.set(core::str::from_utf8(&text[..len]).unwrap_or(""));
}

/// 把无符号整数和单位后缀写进缓冲，返回其中的 UTF-8 片段。
fn format_u32<'a>(value: u32, unit: &str, buffer: &'a mut [u8; 20]) -> &'a str {
    let mut rest = value;
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
    let mut out = 0usize;
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

fn update_switch_text() {
    SWITCH_TEXT.set(if DEMO_SWITCH.get() { "开" } else { "关" });
}

fn reset_parameters() {
    BRIGHTNESS.set(80);
    ANIMATION_INTERVAL.set(20);
    DEMO_SWITCH.set(true);
    SHOW_ITEM.set(true);
    DISABLE_ITEM.set(false);
    let mut buffer = [0u8; 20];
    BRIGHTNESS_TEXT.set(format_u32(BRIGHTNESS.get() as u32, "%", &mut buffer));
    update_switch_text();
    INTERVAL_TEXT.set(format_u32(
        ANIMATION_INTERVAL.get() as u32,
        "ms",
        &mut buffer,
    ));
    update_progress_text(0, false);
}

/// 按「显示条目 / 禁用条目」刷新组件演示页最后一行的状态。
fn apply_component_state<C: CustomPage>(ui: &mut KkUi<C>) {
    let state = if !SHOW_ITEM.get() {
        ItemState::Hidden
    } else if DISABLE_ITEM.get() {
        ItemState::Disabled
    } else {
        ItemState::Normal
    };
    ui.set_menu_item_state(PAGE_COMPONENTS, 3, state);
}

/// 弹 Toast；覆盖层占用时留到下一帧补发。
fn toast<C: CustomPage>(ui: &mut KkUi<C>, text: &'static str, duration: u32) {
    if ui.show_toast(text, duration) == kk_ui::Status::Busy {
        PENDING_TOAST.set(Some((text, duration)));
    } else {
        PENDING_TOAST.set(None);
    }
}

/// 本屏背光接 GND，没有对比度控制：把「屏幕亮度」标成禁用。
fn mark_brightness_unsupported<C: CustomPage>(ui: &mut KkUi<C>) {
    BRIGHTNESS_STATE.set("不支持");
    ui.set_menu_item_state(PAGE_SETTINGS, 0, ItemState::Disabled);
}

/// 屏幕亮度依赖 OLED 的对比度命令；只能如实上报不支持。
fn report_brightness_unsupported<C: CustomPage>(ui: &mut KkUi<C>) {
    mark_brightness_unsupported(ui);
    toast(ui, "亮度不支持", 0);
}

/// 自动演示记录「哪些行打开过」用的页面槽位数，按页面编号取模。
const AUTOPLAY_PAGES: usize = 8;

/// 自动演示：每一拍按当前界面状态挑一个键，没人操作时自己走遍所有页面。
///
/// 刻意不用固定按键脚本：脚本会跟着界面状态漂移（某个条目被禁用后同样的按键
/// 落到别的条目上），最后卡在同一个弹窗里打转。这里只看「弹窗开着吗 / 当前哪一页 /
/// 选中第几行」，所以怎么漂都能自己走出来。
pub struct Autoplay {
    /// 下一个要进的首页图标下标。
    target: u16,
    /// 每个菜单页本轮已经打开过的行（按行号记位）。
    opened: [u32; AUTOPLAY_PAGES],
}

impl Default for Autoplay {
    fn default() -> Self {
        Self::new()
    }
}

impl Autoplay {
    /// 从第一步开始。
    pub const fn new() -> Self {
        Self {
            target: 0,
            opened: [0; AUTOPLAY_PAGES],
        }
    }

    /// 决定这一拍按哪个键。
    pub fn next_key<C: CustomPage>(&mut self, ui: &KkUi<'_, C>) -> u8 {
        // 弹窗一律用「确定」收尾：整数/开关编辑器两下确定提交并关闭，确认框与消息框一下就关。
        if ui.dialog_open() {
            return KEY_OK;
        }
        let page = ui.current_page();
        if page == PAGE_HOME {
            // 首页：先把选中项走到目标图标，再按「确定」进去；目标按顺序轮换。
            if ui.selected_index() == self.target {
                self.target = (self.target + 1) % HOME_ITEMS.len() as u16;
                return KEY_OK;
            }
            return KEY_DOWN;
        }
        let items = menu_item_count(page);
        if items == 0 {
            // 信息页与自绘页都用「确定」返回。
            return KEY_OK;
        }
        // 菜单：每一行先按「确定」打开一次，打开过就往下走；走到最后一行（虚拟返回项）
        // 就清空标记并离开，下一轮进来又是逐个打开。
        let selected = ui.selected_index();
        let slot = page as usize % AUTOPLAY_PAGES;
        if selected < items {
            let bit = 1u32 << (selected & 31);
            if self.opened[slot] & bit == 0 {
                self.opened[slot] |= bit;
                return KEY_OK;
            }
            return KEY_DOWN;
        }
        self.opened[slot] = 0;
        KEY_OK
    }
}

/// 菜单页的条目数（不含虚拟返回项）；非菜单页返回 0。
pub fn menu_item_count(page: PageId) -> u16 {
    match page {
        PAGE_SETTINGS => SETTINGS_ITEMS.len() as u16,
        PAGE_COMPONENTS => COMPONENT_ITEMS.len() as u16,
        _ => 0,
    }
}

/// 初始化应用状态；在 [`KkUi::init`] 之前或之后调用都可以。
pub fn reset<C: CustomPage>(ui: &mut KkUi<C>, now: u32) {
    reset_parameters();
    for slot in &SETTINGS_STATES {
        slot.set(0);
    }
    for slot in &COMPONENT_STATES {
        slot.set(0);
    }
    mark_brightness_unsupported(ui);
    UPTIME.set("0s");
    APP_STARTED_AT.set(now);
    INFO_UPDATED_AT.set(now);
    APP_NOW.set(now);
    UPDATE_EPOCH.set(0);
    PENDING_TOAST.set(None);
    apply_component_state(ui);
    ui.invalidate();
}

/// 每秒刷新一次运行时间。
pub fn update<C: CustomPage>(ui: &mut KkUi<C>, now: u32) {
    APP_NOW.set(now);
    UPDATE_EPOCH.set(UPDATE_EPOCH.get().wrapping_add(1));
    if now.wrapping_sub(INFO_UPDATED_AT.get()) >= 1000 {
        INFO_UPDATED_AT.set(now);
        let seconds = now.wrapping_sub(APP_STARTED_AT.get()) / 1000;
        let mut buffer = [0u8; 20];
        UPTIME.set(format_u32(seconds, "s", &mut buffer));
        ui.invalidate();
    }
}

/// 处理应用事件。
pub fn process_events<C: CustomPage>(ui: &mut KkUi<C>) {
    if let Some((text, duration)) = PENDING_TOAST.get() {
        toast(ui, text, duration);
    }
    while let Some(event) = ui.poll_event() {
        match event {
            EVENT_RESET_CONFIRMED => {
                reset_parameters();
                apply_component_state(ui);
                ui.invalidate();
                report_brightness_unsupported(ui);
            }
            EVENT_SHOW_MESSAGE => {
                ui.show_message("确定关闭", EVENT_MESSAGE_ACKNOWLEDGED);
            }
            EVENT_SHOW_TOAST => toast(ui, "这是 Toast 提示", 0),
            EVENT_SWITCH_CHANGED => {
                update_switch_text();
                ui.invalidate();
                toast(ui, "设置已更新", 0);
            }
            EVENT_INTERVAL_CHANGED => {
                let mut buffer = [0u8; 20];
                INTERVAL_TEXT.set(format_u32(
                    ANIMATION_INTERVAL.get() as u32,
                    "ms",
                    &mut buffer,
                ));
                ui.invalidate();
                toast(ui, "设置已更新", 0);
            }
            EVENT_BRIGHTNESS_CHANGED => {
                let mut buffer = [0u8; 20];
                BRIGHTNESS_TEXT.set(format_u32(BRIGHTNESS.get() as u32, "%", &mut buffer));
                ui.invalidate();
                toast(ui, "亮度不支持", 0);
            }
            EVENT_ADVANCED_VISIBILITY_CHANGED | EVENT_DEMO_LOCK_CHANGED => {
                apply_component_state(ui);
                toast(ui, "设置已更新", 0);
            }
            EVENT_ADVANCED_ACTION => toast(ui, "操作已触发", 0),
            _ => {}
        }
    }
    if !BRIGHTNESS_REPORTED.get() {
        BRIGHTNESS_REPORTED.set(true);
        report_brightness_unsupported(ui);
    }
}

impl CustomPage for Wave {
    fn on_enter(&mut self, _ui: &mut Runtime, _page: PageId) {
        let now = APP_NOW.get();
        self.reset(now);
    }

    fn on_input(&mut self, ui: &mut Runtime, _page: PageId, event: InputEvent) {
        match event.action {
            InputAction::Ok => {
                ui.custom_request_close();
            }
            InputAction::Up => {
                self.manual = true;
                self.progress = self.progress.saturating_sub(event.steps);
                update_progress_text(self.progress, self.manual);
                ui.invalidate();
            }
            InputAction::Down => {
                self.manual = true;
                self.progress = (self.progress + event.steps).min(100);
                update_progress_text(self.progress, self.manual);
                ui.invalidate();
            }
        }
    }

    fn on_tick(&mut self, _ui: &mut Runtime, _page: PageId, now: u32) -> bool {
        let interval = ANIMATION_INTERVAL.get().max(1) as u32;
        let next = (now.wrapping_sub(self.entered_at) / interval % 32) as u16;
        let mut changed = next != self.phase;
        let elapsed = now.wrapping_sub(self.fps_started);
        self.phase = next;
        if !self.manual {
            let progress = (now.wrapping_sub(self.entered_at) / 50 % 101) as u16;
            if progress != self.progress {
                self.progress = progress;
                update_progress_text(self.progress, self.manual);
                changed = true;
            }
        }
        if elapsed >= 1000 {
            let fps = ((self.draw_count * 1000 + elapsed / 2) / elapsed).min(999);
            let mut buffer = [0u8; 20];
            FPS_TEXT.set(format_u32(fps, "", &mut buffer));
            self.draw_count = 0;
            self.fps_started = now;
            changed = true;
        }
        changed
    }

    fn on_draw(
        &mut self,
        ui: &mut Runtime,
        _page: PageId,
        x_offset: i16,
        clip_x: i16,
        clip_width: u16,
    ) {
        if self.draw_epoch != UPDATE_EPOCH.get() {
            self.draw_epoch = UPDATE_EPOCH.get();
            self.draw_count += 1;
        }
        let frame: &mut Frame<'_> = ui.frame_mut();
        let (width, height) = (frame.width(), frame.height());
        frame.set_clip(clip_x, 0, clip_width, height as u16);
        frame.set_font(&FONT_TITLE);
        frame.draw_text(4 + x_offset, 1, "波形");
        frame.draw_text(width - 62 + x_offset, 1, "FPS:");
        FPS_TEXT.with(|text| {
            frame.draw_text(width - 36 + x_offset, 1, text);
        });
        frame.draw_line(width - 11 + x_offset, 5, width - 5 + x_offset, 11);
        frame.draw_line(width - 5 + x_offset, 5, width - 11 + x_offset, 11);
        frame.set_draw_mode(DrawMode::Xor);
        frame.draw_rbox(width - 15 + x_offset, 1, 14, 14, 3);
        frame.set_draw_mode(DrawMode::Set);
        let wave: [i16; 16] = [0, -3, -7, -11, -7, -3, 0, 3, 7, 11, 7, 3, 0, -2, -1, 0];
        let base = height / 2;
        let mut previous_y = base;
        for x in 0..width {
            let index = (x as usize + self.phase as usize) & 15;
            let y = base + wave[index] * 8 / 11;
            if x != 0 {
                frame.draw_line(x - 1 + x_offset, previous_y, x + x_offset, y);
            }
            previous_y = y;
        }
        frame.set_font(&FONT_BODY);
        PROGRESS_TEXT.with(|text| {
            frame.draw_text(10 + x_offset, height - 28, text);
        });
        draw_progress_bar(
            frame,
            10 + x_offset,
            height - 16,
            (width - 20) as u16,
            8,
            self.progress,
            100,
        );
        frame.reset_clip();
    }
}
