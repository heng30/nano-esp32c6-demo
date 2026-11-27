//! 移植自 KK_UI 标准 Demo 的应用层：页面表、业务状态与自绘页。
//!
//! 只依赖 `kk-ui-rs` 与 `core`，可以在主机上原样编入渲染测试工程。
//! 与 `kk-ui-rs/examples/sim/app.rs`、`ili9341-lcd-kkui/src/app.rs` 是同一份内容
//! （图标在各自的 `assets/`），改了一边记得同步另外两边。

mod autoplay;
mod handlers;
mod math;
mod tables;
mod wave;

pub use autoplay::Autoplay;
pub use handlers::{process_events, reset, update};
pub use tables::APP;
