//! KK_UI 的 Rust 移植：单色菜单 UI 运行时。
//!
//! 结构对应上游 C 版：首页图标选择器、可嵌套菜单、只读信息页、应用自绘页
//! （[`CustomPage`]）、整数/开关编辑器、确认框、消息框与 Toast。
//! 画布与 KK_OLED 一致是页式 1 bit 帧缓冲，尺寸由调用方给定（[`Frame::new`]），
//! 由应用负责把它放大送到真实屏幕。
//!
//! 相比 C 版没有 `KK_UI_ENABLE_*` 编译开关：所有页面与弹窗都编译，未用到的部分由
//! release 的 `lto = "fat"` + gc-sections 从最终镜像里剥掉，不必手写 cfg。也没移植
//! 异步 IT/DMA 刷新路径：渲染结果直接落在 [`Frame`]，提交由应用阻塞完成。

#![no_std]

// 模拟器跑在宿主机的 std 上，只有 `sim` 特性才把它拉进来。
#[cfg(feature = "sim")]
extern crate std;

mod font;
mod gfx;
mod model;
mod runtime;
mod widgets;

#[cfg(feature = "cjk-fonts")]
pub mod cjk;
#[cfg(feature = "sim")]
pub mod sim;

pub use font::Font;
pub use gfx::{DrawMode, Frame};
pub use model::{
    App, BoolBinding, ConfirmDesc, EVENT_NONE, EventId, Fonts, HomeItem, HomePage, InfoPage,
    InfoRow, Input, InputAction, InputEvent, InputSource, IntBinding, ItemState, KEY_DOWN, KEY_OK,
    KEY_UP, MenuItem, MenuItemType, MenuPage, PAGE_NONE, PageId, PageRoute, PageType, Shared, Text,
    Texts,
};
pub use runtime::{CustomPage, ErrorInfo, KkUi, Runtime, Status};
pub use widgets::{draw_focus, draw_progress_bar, draw_switch, utils::format_int};
