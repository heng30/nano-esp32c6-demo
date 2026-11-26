//! 上位机 ↔ 板子的帧协议实现，按职责拆成几个子模块。
//!
//! - [`protocol`]：信封、命令/应答常量、读消息、应答组包
//! - [`pool`]：消息槽池、待渲染队列、进度快照、当前采样倍数
//! - [`tile`]：一块 tile 的布局解析、四种编码解码、铺回 16×16 并写屏
//! - [`render`]：渲染任务（单一所有者画屏）、帧进度与帧率统计
//! - [`serve`]：一条链路的收帧循环（数据入队、控制命令一问一答）
//!
//! 协议常量与上位机 `secondary-screen-desktop/src/link.rs` 一一对应，两边必须一起改。

mod pool;
mod protocol;
mod render;
mod serve;
mod tile;

pub use pool::init_pool;
pub use render::render;
pub use serve::serve;
