//! 应用描述表：路由、页面、绑定、字体与文案。
//!
//! 与 C 版一致，全部是编译期常量表，UI 运行时只读；应用的可变状态通过
//! [`Shared`] 与 [`Text`] 这类共享单元暴露给表项。

use core::cell::Cell;

use crate::font::Font;

/// 页面编号，`0` 表示无页面。
pub type PageId = u16;
/// 事件编号，`0` 表示无事件。
pub type EventId = u16;

/// 无页面。
pub const PAGE_NONE: PageId = 0;
/// 无事件。
pub const EVENT_NONE: EventId = 0;

/// 按键位：上。
pub const KEY_UP: u8 = 1 << 0;
/// 按键位：下。
pub const KEY_DOWN: u8 = 1 << 1;
/// 按键位：确定。
pub const KEY_OK: u8 = 1 << 2;

/// 运行时文本缓冲的最大字节数，按 UTF-8 计；32 字节能放下 10 个汉字。
pub const TEXT_LEN: usize = 32;

/// 单核裸机下的共享可变值：只在 UI 主循环与其回调里访问。
pub struct Shared<T>(Cell<T>);

// SAFETY: 只在单核上由 UI 主循环顺序访问，不跨核、不在中断里使用。
unsafe impl<T> Sync for Shared<T> {}

impl<T: Copy> Shared<T> {
    /// 创建初值。
    pub const fn new(value: T) -> Self {
        Self(Cell::new(value))
    }

    /// 读取当前值。
    pub fn get(&self) -> T {
        self.0.get()
    }

    /// 写入新值。
    pub fn set(&self, value: T) {
        self.0.set(value)
    }
}

/// 长度受限的共享文本，供会变化的表项（如运行时间、亮度）使用。
pub struct Text {
    /// 内容字节，未用部分补 0。
    bytes: Cell<[u8; TEXT_LEN]>,
    /// 有效字节数，按 UTF-8 计。
    len: Cell<u8>,
}

// SAFETY: 与 [`Shared`] 相同，只在单核 UI 主循环里顺序访问。
unsafe impl Sync for Text {}

impl Default for Text {
    /// 空文本。
    fn default() -> Self {
        Self::new()
    }
}

impl Text {
    /// 空文本。
    pub const fn new() -> Self {
        Self {
            bytes: Cell::new([0; TEXT_LEN]),
            len: Cell::new(0),
        }
    }

    /// 由字符串字面量构造；超出 [`TEXT_LEN`] 的部分按字符边界截断。
    pub const fn literal(text: &str) -> Self {
        let source = text.as_bytes();
        let len = fit(source);
        let mut bytes = [0u8; TEXT_LEN];
        let mut index = 0;
        while index < len {
            bytes[index] = source[index];
            index += 1;
        }
        Self {
            bytes: Cell::new(bytes),
            len: Cell::new(len as u8),
        }
    }

    /// 覆盖内容；超出 [`TEXT_LEN`] 的部分按字符边界截断。
    pub fn set(&self, text: &str) {
        let source = text.as_bytes();
        let mut bytes = [0u8; TEXT_LEN];
        let len = fit(source);
        bytes[..len].copy_from_slice(&source[..len]);
        self.bytes.set(bytes);
        self.len.set(len as u8);
    }

    /// 在栈上复制一份内容后交给闭包，返回值原样透传。
    pub fn with<R>(&self, f: impl FnOnce(&str) -> R) -> R {
        let bytes = self.bytes.get();
        let len = self.len.get() as usize;
        f(core::str::from_utf8(&bytes[..len]).unwrap_or(""))
    }
}

/// 不超过 [`TEXT_LEN`] 且落在字符边界上的字节数。
const fn fit(source: &[u8]) -> usize {
    let mut len = if source.len() < TEXT_LEN {
        source.len()
    } else {
        TEXT_LEN
    };
    while len > 0 && len < source.len() && source[len] & 0xC0 == 0x80 {
        len -= 1;
    }
    len
}

/// 一次刷新传入的原始输入。
#[derive(Clone, Copy, Default)]
pub struct Input {
    /// 按键位掩码，取 [`KEY_UP`] / [`KEY_DOWN`] / [`KEY_OK`] 的组合。
    pub keys: u8,
    /// 编码器增量，正值为向下、负值为向上。
    pub encoder_delta: i16,
}

/// 归一化后的输入动作。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    /// 向上。
    Up,
    /// 向下。
    Down,
    /// 确定。
    Ok,
}

/// 输入来源。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputSource {
    /// 首次按下。
    Press,
    /// 长按重复。
    Repeat,
    /// 编码器。
    Encoder,
}

/// 分发给页面或弹窗的输入事件。
#[derive(Clone, Copy)]
pub struct InputEvent {
    /// 动作。
    pub action: InputAction,
    /// 来源。
    pub source: InputSource,
    /// 一次性合并的步数。
    pub steps: u16,
}

/// 页面类型。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PageType {
    /// 首页图标选择器。
    Home,
    /// 菜单。
    Menu,
    /// 只读信息页。
    Info,
    /// 应用自绘页。
    Custom,
}

/// 页面编号到页面表下标的映射。
#[derive(Clone, Copy)]
pub struct PageRoute {
    /// 页面类型。
    pub kind: PageType,
    /// 对应页面表的下标。
    pub index: u16,
}

/// 首页的一个图标入口。
pub struct HomeItem {
    /// 文字标签。
    pub label: &'static str,
    /// 32×32 XBM 图标。
    pub icon: &'static [u8],
    /// 选中后进入的页面编号。
    pub target: PageId,
}

/// 一组首页入口。
pub struct HomePage {
    /// 入口表。
    pub items: &'static [HomeItem],
}

/// 菜单项类型。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MenuItemType {
    /// 进入子页面。
    Page,
    /// 触发事件。
    Action,
    /// 整数编辑器。
    Int,
    /// 开关编辑器。
    Bool,
    /// 确认框。
    Confirm,
}

/// 菜单项。
pub struct MenuItem {
    /// 标签。
    pub label: &'static str,
    /// 类型。
    pub kind: MenuItemType,
    /// 页面编号、事件编号或绑定下标，含义由类型决定。
    pub reference: u16,
}

/// 菜单页。
pub struct MenuPage {
    /// 标题。
    pub title: &'static str,
    /// 条目表。
    pub items: &'static [MenuItem],
    /// 可选的条目状态表，每字节压 4 个 2 bit 状态。
    pub states: Option<&'static [Shared<u8>]>,
}

/// 条目状态。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ItemState {
    /// 正常。
    Normal = 0,
    /// 隐藏。
    Hidden = 1,
    /// 显示但不可选。
    Disabled = 2,
    /// 保留值。
    Reserved = 3,
}

/// 信息页的一行。
pub struct InfoRow {
    /// 名称。
    pub name: &'static str,
    /// 取值；空文本表示不绘制右列。
    pub value: &'static Text,
}

/// 信息页。
pub struct InfoPage {
    /// 标题；空表示不画标题栏。
    pub title: &'static str,
    /// 行表。
    pub rows: &'static [InfoRow],
}

/// 整数编辑器绑定。
pub struct IntBinding {
    /// 标题。
    pub title: &'static str,
    /// 绑定的值。
    pub value: &'static Shared<i32>,
    /// 最小值。
    pub minimum: i32,
    /// 最大值。
    pub maximum: i32,
    /// 单步增量。
    pub step: u32,
    /// 单位后缀。
    pub unit: &'static str,
    /// 提交后入队的事件。
    pub changed_event: EventId,
}

/// 开关编辑器绑定。
pub struct BoolBinding {
    /// 标题。
    pub title: &'static str,
    /// 绑定的值。
    pub value: &'static Shared<bool>,
    /// 提交后入队的事件。
    pub changed_event: EventId,
}

/// 确认框描述。
pub struct ConfirmDesc {
    /// 正文，可用 `\n` 分成两行。
    pub text: &'static str,
    /// 确认事件。
    pub confirmed_event: EventId,
    /// 取消事件。
    pub cancelled_event: EventId,
}

/// 三档字体。
pub struct Fonts {
    /// 首页图标下方标签。
    pub home: &'static Font,
    /// 标题。
    pub title: &'static Font,
    /// 正文。
    pub body: &'static Font,
}

/// 固定文案。
pub struct Texts {
    /// 菜单里的返回项。
    pub return_text: &'static str,
    /// 取消。
    pub cancel_text: &'static str,
    /// 确定。
    pub confirm_text: &'static str,
    /// 开。
    pub on_text: &'static str,
    /// 关。
    pub off_text: &'static str,
    /// 消息框标题。
    pub message_title: &'static str,
}

/// 整个应用描述。
pub struct App {
    /// 启动页编号。
    pub root_page: PageId,
    /// 路由表。
    pub routes: &'static [PageRoute],
    /// 首页入口表。
    pub home_pages: &'static [HomePage],
    /// 菜单页表。
    pub menu_pages: &'static [MenuPage],
    /// 信息页表。
    pub info_pages: &'static [InfoPage],
    /// 自绘页数量。
    pub custom_page_count: u16,
    /// 整数绑定表。
    pub int_bindings: &'static [IntBinding],
    /// 开关绑定表。
    pub bool_bindings: &'static [BoolBinding],
    /// 确认框表。
    pub confirm_descs: &'static [ConfirmDesc],
    /// 字体。
    pub fonts: Fonts,
    /// 文案。
    pub texts: Texts,
}
