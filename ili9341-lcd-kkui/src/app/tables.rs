//! 应用描述表：路由、页面数据、绑定、事件编号与静态文案。
//!
//! 页面之间用 `&ROUTE_XXX` 句柄互相引用，`App.routes` 只是供初始化校验的无序清单。

use kk_ui::{
    App, BoolBinding, ConfirmDesc, HomeItem, HomePage, InfoPage, InfoRow, IntBinding, MenuAction,
    MenuItem, MenuPage, PageRoute, Texts,
};

use super::math::MathPage;
use super::wave::Wave;

kk_ui::event_ids! {
    /// 事件：亮度改变。
    EVENT_BRIGHTNESS_CHANGED,
    /// 事件：开关改变。
    EVENT_SWITCH_CHANGED,
    /// 事件：动画间隔改变。
    EVENT_INTERVAL_CHANGED,
    /// 事件：确认恢复默认。
    EVENT_RESET_CONFIRMED,
    /// 事件：取消恢复默认。
    EVENT_RESET_CANCELLED,
    /// 事件：弹出消息框。
    EVENT_SHOW_MESSAGE,
    /// 事件：消息框已确认。
    EVENT_MESSAGE_ACKNOWLEDGED,
    /// 事件：条目可见性改变。
    EVENT_ADVANCED_VISIBILITY_CHANGED,
    /// 事件：条目禁用改变。
    EVENT_DEMO_LOCK_CHANGED,
    /// 事件：普通操作示例。
    EVENT_ADVANCED_ACTION,
    /// 事件：弹出 Toast。
    EVENT_SHOW_TOAST,
}

// 图标是 32×32 XBM，每个 128 字节。
kk_ui::icons! {
    ICON_SETTINGS = "../assets/icon0.bin",
    ICON_MONITOR = "../assets/icon1.bin",
    ICON_WAVE = "../assets/icon2.bin",
    ICON_ABOUT = "../assets/icon3.bin",
    ICON_MATH = "../assets/math.bin",
}

kk_ui::shared! {
    BRIGHTNESS: i32 = 80,
    ANIMATION_INTERVAL: i32 = 20,
    DEMO_SWITCH: bool = true,
    SHOW_ITEM: bool = true,
    DISABLE_ITEM: bool = false,
}
kk_ui::texts! {
    UPTIME = "0s",
    BRIGHTNESS_TEXT = "80%",
    SWITCH_TEXT = "开",
    INTERVAL_TEXT = "20ms",
    BRIGHTNESS_STATE = "等待",
    SCREEN_INFO = "160x120@2x",
    DRIVER_INFO = "kk-ui-rs",
    VERSION_INFO = "0.2.0",
    DEVICE_INFO = "KK_UI",
    REFRESH_INFO = "Blocking",
    YES_INFO = "否",
    DPAD_INFO = "选择/调整",
    OK_INFO = "进入/确认",
    EXIT_INFO = "确定退出",
    FPS_INFO = "波形绘制",
    FPS_TEXT = "--",
    PROGRESS_TEXT = "自动 0%",
}

kk_ui::shared! {
    APP_STARTED_AT: u32 = 0,
    INFO_UPDATED_AT: u32 = 0,
    APP_NOW: u32 = 0,
    UPDATE_EPOCH: u32 = 0,
    /// 待补发的 Toast：覆盖层占用时先记下，下一帧再试。
    PENDING_TOAST: Option<(&'static str, u32)> = None,
    /// 亮度控制只上报一次「不支持」。
    BRIGHTNESS_REPORTED: bool = false,
}

kk_ui::menu_states! {
    /// 设置页条目状态（压缩成 2 bit 一项）。
    SETTINGS_STATES = [0, 0],
    /// 组件演示页条目状态。
    COMPONENT_STATES = [0, 0],
}

/// 页面：首页。
pub(super) static ROUTE_HOME: PageRoute = PageRoute::Home(&HomePage { items: &HOME_ITEMS });
/// 页面：设置。
pub(super) static ROUTE_SETTINGS: PageRoute = PageRoute::Menu(&MenuPage {
    title: "设置",
    items: &SETTINGS_ITEMS,
    states: Some(&SETTINGS_STATES),
});
/// 页面：数据监测。
pub(super) static ROUTE_MONITOR: PageRoute = PageRoute::Info(&InfoPage {
    title: "数据监测",
    rows: &MONITOR_ROWS,
});
/// 页面：波形。
pub(super) static ROUTE_WAVE: PageRoute = PageRoute::Custom(&WAVE);
/// 页面：关于设备。
pub(super) static ROUTE_ABOUT: PageRoute = PageRoute::Info(&InfoPage {
    title: "关于设备",
    rows: &ABOUT_ROWS,
});
/// 页面：组件演示。
pub(super) static ROUTE_COMPONENTS: PageRoute = PageRoute::Menu(&MenuPage {
    title: "组件演示",
    items: &COMPONENT_ITEMS,
    states: Some(&COMPONENT_STATES),
});
/// 页面：数学。
pub(super) static ROUTE_MATH: PageRoute = PageRoute::Custom(&MATH);

/// 整数编辑器绑定：屏幕亮度。
pub(super) static INT_BRIGHTNESS: IntBinding = IntBinding {
    title: "屏幕亮度",
    value: &BRIGHTNESS,
    minimum: 10,
    maximum: 100,
    step: 1,
    unit: "%",
    changed_event: EVENT_BRIGHTNESS_CHANGED,
};
/// 整数编辑器绑定：动画间隔。
pub(super) static INT_INTERVAL: IntBinding = IntBinding {
    title: "动画间隔",
    value: &ANIMATION_INTERVAL,
    minimum: 10,
    maximum: 100,
    step: 10,
    unit: "ms",
    changed_event: EVENT_INTERVAL_CHANGED,
};

/// 开关编辑器绑定：开关。
pub(super) static BOOL_SWITCH: BoolBinding = BoolBinding {
    title: "开关",
    value: &DEMO_SWITCH,
    changed_event: EVENT_SWITCH_CHANGED,
};
/// 开关编辑器绑定：显示条目。
pub(super) static BOOL_SHOW_ITEM: BoolBinding = BoolBinding {
    title: "显示条目",
    value: &SHOW_ITEM,
    changed_event: EVENT_ADVANCED_VISIBILITY_CHANGED,
};
/// 开关编辑器绑定：禁用条目。
pub(super) static BOOL_DISABLE_ITEM: BoolBinding = BoolBinding {
    title: "禁用条目",
    value: &DISABLE_ITEM,
    changed_event: EVENT_DEMO_LOCK_CHANGED,
};

/// 确认框：恢复默认设置。
pub(super) static CONFIRM_RESET: ConfirmDesc = ConfirmDesc {
    text: "恢复默认设置？\n包含屏幕亮度",
    confirmed_event: EVENT_RESET_CONFIRMED,
    cancelled_event: EVENT_RESET_CANCELLED,
};

pub(super) static HOME_ITEMS: [HomeItem; 5] = [
    HomeItem {
        label: "设置",
        icon: ICON_SETTINGS,
        target: &ROUTE_SETTINGS,
    },
    HomeItem {
        label: "数据监测",
        icon: ICON_MONITOR,
        target: &ROUTE_MONITOR,
    },
    HomeItem {
        label: "波形",
        icon: ICON_WAVE,
        target: &ROUTE_WAVE,
    },
    HomeItem {
        label: "数学",
        icon: ICON_MATH,
        target: &ROUTE_MATH,
    },
    HomeItem {
        label: "关于设备",
        icon: ICON_ABOUT,
        target: &ROUTE_ABOUT,
    },
];

pub(super) static SETTINGS_ITEMS: [MenuItem; 6] = [
    MenuItem {
        label: "屏幕亮度",
        action: MenuAction::Int(&INT_BRIGHTNESS),
    },
    MenuItem {
        label: "开关",
        action: MenuAction::Bool(&BOOL_SWITCH),
    },
    MenuItem {
        label: "组件演示",
        action: MenuAction::Page(&ROUTE_COMPONENTS),
    },
    MenuItem {
        label: "确认框",
        action: MenuAction::Confirm(&CONFIRM_RESET),
    },
    MenuItem {
        label: "消息框",
        action: MenuAction::Event(EVENT_SHOW_MESSAGE),
    },
    MenuItem {
        label: "Toast",
        action: MenuAction::Event(EVENT_SHOW_TOAST),
    },
];

pub(super) static COMPONENT_ITEMS: [MenuItem; 4] = [
    MenuItem {
        label: "动画间隔",
        action: MenuAction::Int(&INT_INTERVAL),
    },
    MenuItem {
        label: "显示条目",
        action: MenuAction::Bool(&BOOL_SHOW_ITEM),
    },
    MenuItem {
        label: "禁用条目",
        action: MenuAction::Bool(&BOOL_DISABLE_ITEM),
    },
    MenuItem {
        label: "操作示例",
        action: MenuAction::Event(EVENT_ADVANCED_ACTION),
    },
];

pub(super) static MONITOR_ROWS: [InfoRow; 5] = [
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

pub(super) static ABOUT_ROWS: [InfoRow; 10] = [
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

pub(super) static INT_BINDINGS: [&IntBinding; 2] = [&INT_BRIGHTNESS, &INT_INTERVAL];
pub(super) static BOOL_BINDINGS: [&BoolBinding; 3] =
    [&BOOL_SWITCH, &BOOL_SHOW_ITEM, &BOOL_DISABLE_ITEM];
pub(super) static CONFIRM_DESCS: [&ConfirmDesc; 1] = [&CONFIRM_RESET];

pub(super) static WAVE: Wave = Wave::new();
pub(super) static MATH: MathPage = MathPage::new();

pub(super) static ROUTES: [&PageRoute; 7] = [
    &ROUTE_HOME,
    &ROUTE_SETTINGS,
    &ROUTE_MONITOR,
    &ROUTE_WAVE,
    &ROUTE_ABOUT,
    &ROUTE_COMPONENTS,
    &ROUTE_MATH,
];

/// 整个应用描述表。
pub static APP: App = App {
    root_page: &ROUTE_HOME,
    routes: &ROUTES,
    int_bindings: &INT_BINDINGS,
    bool_bindings: &BOOL_BINDINGS,
    line_inputs: &[],
    confirm_descs: &CONFIRM_DESCS,
    texts: Texts {
        return_text: "返回",
        cancel_text: "取消",
        confirm_text: "确定",
        on_text: "开",
        off_text: "关",
        message_title: "这是消息框",
    },
};
