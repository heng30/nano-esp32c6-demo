//! 可复用组件：图元与弹窗。
//!
//! 一个组件一个文件，全部只依赖 [`crate::gfx::Frame`]；复合组件（弹窗）自带子目录，
//! 由子目录的 `mod.rs` 会话状态与分发，具体弹窗按类型分文件。

pub mod dialog;

pub mod pages;
pub(crate) mod utils;

mod focus;
mod progress_bar;
mod switch;

pub use focus::draw_focus;
pub use progress_bar::draw_progress_bar;
pub use switch::draw_switch;
