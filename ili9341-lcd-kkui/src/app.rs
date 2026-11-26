//! 移植自 KK_UI 标准 Demo 的应用层：页面表、业务状态与自绘页。
//!
//! 只依赖 `kk-ui-rs` 与 `core`，来自 `kk-ui-rs` 0.2 的 `examples/sim/app.rs`，
//! 与上游保持同构：页面表在 `app/tables.rs`，事件处理在 `app/handlers.rs`，
//! 自绘页一个文件一个（`app/wave.rs`、`app/math.rs`）。

mod autoplay;
mod handlers;
mod math;
mod tables;
mod wave;

pub use autoplay::Autoplay;
pub use handlers::{process_events, reset, update};
pub use tables::APP;
