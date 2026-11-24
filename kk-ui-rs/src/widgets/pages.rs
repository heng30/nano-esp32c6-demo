//! 三类页面：首页图标选择器、菜单、只读信息页。
//!
//! 一类页面一个文件，`mod.rs` 只汇总对外入口；调用方拿到的是与运行时无关的
//! `animate`/`input`/`draw` 三件套。

mod home;
mod info;
mod menu;

pub(crate) use home::{home_animate, home_draw, home_input};
pub(crate) use info::{info_animate, info_draw, info_input};
pub(crate) use menu::{menu_animate, menu_draw, menu_input, menu_repair_focus};
