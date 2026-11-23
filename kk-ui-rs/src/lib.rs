//! KK_UI 的 Rust 移植：128×64 单色菜单 UI 运行时。
//!
//! 结构对应上游 C 版：首页图标选择器、可嵌套菜单、只读信息页、应用自绘页
//! （[`CustomPage`]）、整数/开关编辑器、确认框、消息框与 Toast。
//! 画布与 KK_OLED 一致是页式 1 bit 帧缓冲，由应用负责把它放大送到真实屏幕。
//!
//! 相比 C 版，这里没有 `KK_UI_ENABLE_*` 编译开关（全部功能都编译进来），也没有
//! 异步 IT/DMA 刷新路径：渲染结果直接落在 [`Frame`]，提交由应用阻塞完成。

#![no_std]

mod dialog;
mod font;
mod gfx;
mod helpers;
mod model;
mod pages;
mod runtime;

pub use dialog::format_int;
pub use font::Font;
pub use gfx::{DrawMode, Frame, HEIGHT, WIDTH};
pub use helpers::{draw_focus, draw_progress_bar, draw_switch};
pub use model::{
    App, BoolBinding, ConfirmDesc, EVENT_NONE, EventId, Fonts, HomeItem, HomePage, InfoPage,
    InfoRow, Input, InputAction, InputEvent, InputSource, IntBinding, ItemState, KEY_DOWN, KEY_OK,
    KEY_UP, MenuItem, MenuItemType, MenuPage, PAGE_NONE, PageId, PageRoute, PageType, Shared, Text,
    Texts,
};
pub use runtime::{CustomPage, ErrorInfo, KkUi, Runtime, Status};
