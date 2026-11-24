//! 弹窗交互：结束编辑后焦点要真的落到确认键，确认框能用方向键切换焦点。
//!
//! 这两条曾经是坏的：`set_focus` 只补间了焦点框，没记住新的焦点模式，
//! 于是编辑器按「确定」永远回到编辑态、确认框永远停在取消上。

use kk_ui::{
    App, BoolBinding, ConfirmDesc, CustomPage, Font, Frame, Input, IntBinding, KEY_DOWN, KEY_OK,
    KkUi, MenuItem, MenuItemType, MenuPage, PageId, PageRoute, PageType, Shared, Texts, set_font,
};

/// 测试用页面：首页 → 菜单。
const PAGE_HOME: PageId = 1;
const PAGE_MENU: PageId = 2;
/// 编辑器/确认框提交后入队的事件号。
const EVENT_INT: u16 = 1;
const EVENT_BOOL: u16 = 2;
const EVENT_CONFIRMED: u16 = 3;
const EVENT_CANCELLED: u16 = 4;
/// 画布尺寸。
const WIDTH: i16 = 128;
const HEIGHT: i16 = 64;

/// 只用到头部字段的假字体：字模查不到，文字绘制为空。
static FONT: Font = Font::new(&[
    0, 0, 2, 1, 4, 4, 3, 3, 3, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]);
static ICON: [u8; 8] = [0; 8];
static INT_VALUE: Shared<i32> = Shared::new(10);
static BOOL_VALUE: Shared<bool> = Shared::new(false);

struct NoPage;

impl CustomPage for NoPage {}

static ROUTES: [PageRoute; 2] = [
    PageRoute {
        kind: PageType::Home,
        index: 0,
    },
    PageRoute {
        kind: PageType::Menu,
        index: 0,
    },
];

static HOME_ITEMS: [kk_ui::HomeItem; 1] = [kk_ui::HomeItem {
    label: "Menu",
    icon: &ICON,
    target: PAGE_MENU,
}];
static HOME_PAGES: [kk_ui::HomePage; 1] = [kk_ui::HomePage { items: &HOME_ITEMS }];

static MENU_ITEMS: [MenuItem; 3] = [
    MenuItem {
        label: "Level",
        kind: MenuItemType::Int,
        reference: 0,
    },
    MenuItem {
        label: "Switch",
        kind: MenuItemType::Bool,
        reference: 0,
    },
    MenuItem {
        label: "Reset",
        kind: MenuItemType::Confirm,
        reference: 0,
    },
];
static MENU_PAGES: [MenuPage; 1] = [MenuPage {
    title: "Menu",
    items: &MENU_ITEMS,
    states: None,
}];
static INT_BINDINGS: [IntBinding; 1] = [IntBinding {
    title: "Level",
    value: &INT_VALUE,
    minimum: 0,
    maximum: 100,
    step: 1,
    unit: "",
    changed_event: EVENT_INT,
}];
static BOOL_BINDINGS: [BoolBinding; 1] = [BoolBinding {
    title: "Switch",
    value: &BOOL_VALUE,
    changed_event: EVENT_BOOL,
}];
static CONFIRMS: [ConfirmDesc; 1] = [ConfirmDesc {
    text: "Reset?",
    confirmed_event: EVENT_CONFIRMED,
    cancelled_event: EVENT_CANCELLED,
}];

static APP: App = App {
    root_page: PAGE_HOME,
    routes: &ROUTES,
    home_pages: &HOME_PAGES,
    menu_pages: &MENU_PAGES,
    info_pages: &[],
    custom_page_count: 0,
    int_bindings: &INT_BINDINGS,
    bool_bindings: &BOOL_BINDINGS,
    confirm_descs: &CONFIRMS,
    texts: Texts {
        return_text: "Back",
        cancel_text: "Cancel",
        confirm_text: "OK",
        on_text: "On",
        off_text: "Off",
        message_title: "Message",
    },
};

/// 一台只有按键的模拟面板。
struct Panel {
    ui: KkUi<'static, NoPage>,
    now: u32,
}

impl Panel {
    fn new() -> Self {
        // 测试不开 cjk-fonts，默认无字体；装上假 u8g2 字体让布局走真实代码路径。
        set_font(&FONT);
        let buffer: &'static mut [u8] =
            Box::leak(vec![0u8; Frame::buffer_len(WIDTH, HEIGHT)].into_boxed_slice());
        let mut ui = KkUi::new(&APP, NoPage, buffer, WIDTH, HEIGHT);
        assert_eq!(ui.init(), kk_ui::Status::Ok);
        INT_VALUE.set(10);
        BOOL_VALUE.set(false);
        let mut panel = Self { ui, now: 0 };
        panel.step(0);
        panel
    }

    fn step(&mut self, keys: u8) {
        self.now += 5;
        self.ui.update(
            self.now,
            Input {
                keys,
                encoder_delta: 0,
            },
        );
    }

    /// 按下并松开一次按键（按住 40 ms，超过 20 ms 消抖），然后空转到动画跑完。
    fn press(&mut self, keys: u8) {
        for _ in 0..8 {
            self.step(keys);
        }
        for _ in 0..70 {
            self.step(0);
        }
    }
}

#[test]
fn int_editor_commits_after_ending_edit() {
    let mut panel = Panel::new();
    panel.press(KEY_OK); // 首页 → 菜单
    panel.press(KEY_OK); // 打开整数编辑器
    assert_eq!(panel.ui.get_int_editor_draft(0), Some(10));
    panel.press(KEY_DOWN); // 值减一
    assert_eq!(panel.ui.get_int_editor_draft(0), Some(9));
    panel.press(KEY_OK); // 结束编辑，焦点应落到确认键
    panel.press(KEY_OK); // 提交并关闭
    assert_eq!(panel.ui.get_int_editor_draft(0), None, "editor must close");
    assert_eq!(INT_VALUE.get(), 9, "committed value");
    assert_eq!(panel.ui.poll_event(), Some(EVENT_INT));
}

#[test]
fn confirm_dialog_follows_focus() {
    let mut panel = Panel::new();
    panel.press(KEY_OK); // 首页 → 菜单
    panel.press(KEY_DOWN); // 选中开关
    panel.press(KEY_DOWN); // 选中确认框
    panel.press(KEY_OK); // 打开确认框，初始焦点在取消
    panel.press(KEY_DOWN); // 焦点切到确认
    panel.press(KEY_OK);
    assert_eq!(panel.ui.poll_event(), Some(EVENT_CONFIRMED));
}

#[test]
fn confirm_dialog_cancels_from_default_focus() {
    let mut panel = Panel::new();
    panel.press(KEY_OK);
    panel.press(KEY_DOWN);
    panel.press(KEY_DOWN);
    panel.press(KEY_OK);
    panel.press(KEY_OK);
    assert_eq!(panel.ui.poll_event(), Some(EVENT_CANCELLED));
}

#[test]
fn bool_editor_commits_and_toggles() {
    let mut panel = Panel::new();
    panel.press(KEY_OK);
    panel.press(KEY_DOWN); // 选中开关
    panel.press(KEY_OK); // 打开开关编辑器
    panel.press(KEY_DOWN); // 翻转
    panel.press(KEY_OK); // 结束编辑
    panel.press(KEY_OK); // 提交
    assert!(BOOL_VALUE.get());
    assert_eq!(panel.ui.poll_event(), Some(EVENT_BOOL));
}
