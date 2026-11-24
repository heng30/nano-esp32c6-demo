# kk-ui-rs

[KK_UI](https://gitee.com/keysking/kk_ui) 的 Rust 移植：单色菜单 UI 运行时。
提供首页图标选择器、可嵌套菜单、只读信息页、应用自绘页、整数/开关编辑器、确认框、
消息框与 Toast，动画曲线与 C 版一致。画布尺寸由应用决定（C 版写死的 128×64 只是
其中一个取值），布局固定 14/16 px 行高，屏越高菜单能显示的行越多。

```text
应用页面与业务（本 crate 之外）
      ↓
   kk-ui-rs      ← 本 crate：导航、交互、动画、绘制
      ↓
 1 bit 帧缓冲    ← Frame::buffer()
      ↓
显示驱动（由应用实现）
```

## 与 C 版的差异

- **画布不变，输出方式变了**：仍是 KK_OLED 那套页式 1 bit 缓冲（每字节同列 8 行），
  但没有 OLED 控制器和 I2C/SPI 刷新路径。渲染结果直接落在 `Frame` 里，由应用自己
  放大/上色/送到屏幕。
- **没有 `KK_UI_ENABLE_*` 编译开关**：所有页面与弹窗都编译，没被调用的由 release 的
  `lto = "fat"` + gc-sections 从最终镜像里剥掉（实测未被引用的 `Frame::fill_circle`
  在任何符号表里都不出现），所以不必照抄 C 版的开关。真要迁过来，`#[cfg]` 掉
  `App` 的某个字段或 `PageType` 的某个变体就会让每个 `App` 字面量与 `match` 跟着
  变形，收益又已经被 LTO 提前吃掉了。
- **没有 IT/DMA 异步刷新与显示故障恢复**：提交是应用侧的阻塞操作，`DisplayError` /
  `DisplayTimeout` 这类状态与 `RecoverDisplay` 不再存在；`is_display_idle()` 仍在，
  语义简化为「没有脏帧、没有待提交帧」。
- **全局单例变成结构体**：C 版的 `kk_ui` 全局变量改为 `Runtime`，自绘页通过
  `CustomPage` 回调拿到 `&mut Runtime`。C 版回调里能直接调用的 `KK_UI_*` 接口
  在 Rust 里是 `Runtime` 的方法，回调期间会自动转成延迟请求（与 C 版的
  `in_callback` 一致），所以 `on_draw` 里调 `show_toast` 也安全。
- **表项里的可变状态**：C 版用裸指针（`int32_t *`、`uint8_t *`），Rust 用 `Shared<T>`
  和 `Text`（长度受限的共享文本）。它们是单核裸机下顺序访问的 `Cell` 包装。
- **画布尺寸不再固定**：C 版写死 `KK_UI_SCREEN_WIDTH/HEIGHT`，这里由
  `KkUi::new` 的 `width`/`height` 决定（仍无 `alloc`，帧缓冲是调用方给的切片）。
  行高 16、菜单表头 14、信息页表头 16 仍是常量：屏越高菜单越多行，首页图标组与弹窗
  按画布高度垂直居中，Toast 贴底，弹窗与 Toast 横向居中，跨页动画的位移跟着宽度走。
- **文字只实现 UI 实际用到的模式**：u8g2 字库仍是原格式（可以继续用 LEDFont 生成），
  但只支持顶部对齐 + `REF_ALL` 参考高度；旋转/基线/居中参考高度没有移植。
- **图元只移植用到的**：像素、横竖线、直线、矩形框/实心矩形、圆角框/实心圆角矩形、
  圆、实心圆、XBM、UTF-8 文字。椭圆、圆弧、三角形、旋转位图没移植。

## 依赖

只依赖 `core`，没有外部 crate，也没有 `alloc`。可以在主机上直接 `cargo build` 跑
渲染测试（`#![no_std]` 但不用任何外设）。

## 用法

```rust
use kk_ui::{App, CustomPage, Input, KkUi, Status};

// 帧缓冲由应用提供，大小是 `Frame::buffer_len(width, height)`。
const WIDTH: i16 = 128;
const HEIGHT: i16 = 64;
let mut buffer = [0u8; Frame::buffer_len(WIDTH, HEIGHT)];
let mut ui = KkUi::new(&MY_APP, MyCustomPage::new(), &mut buffer, WIDTH, HEIGHT);
ui.init();

loop {
    let now_ms = /* 单调毫秒 */;
    ui.update(now_ms, Input { keys: 0, encoder_delta: 0 });
    if ui.take_frame_ready() {
        blit(ui.frame()); // 把 `width × height` 的 1 bit 画布送到屏幕
    }
}
```

应用描述表（`App`）是编译期常量：路由、页面、绑定、字体与文案，结构对应 C 版
`KK_UI_App`。示例见 `../ili9341-lcd-kkui/src/app.rs`（移植自官方标准 Demo）。

自绘页实现 `CustomPage`：

```rust
impl CustomPage for MyPage {
    fn on_enter(&mut self, ui: &mut Runtime, page: PageId) {}
    fn on_input(&mut self, ui: &mut Runtime, page: PageId, event: InputEvent) {}
    fn on_tick(&mut self, ui: &mut Runtime, page: PageId, now: u32) -> bool { false }
    fn on_draw(&mut self, ui: &mut Runtime, page: PageId, x: i16, clip_x: i16, clip_w: u16) {
        let frame = ui.frame_mut();
        let (width, height) = (frame.width(), frame.height());
        // frame.draw_text / draw_rbox / ...
    }
}
```

## 本地仿真（宿主机）

`sim` 特性把画布接到 eframe 窗口，不用板子也能看效果。库本体仍是 `#![no_std]`，
不开这个特性时 `eframe` / `png` 都不会被拉进来：

```bash
cd kk-ui-rs
cargo run --example sim --features sim          # 窗口：上方画布，下方 上/下/确定/截图
cargo run --example sim --features sim -- --snapshot out.png --steps 40
```

窗口固定大小、内容居中，下方一排大号按钮（上/下/确定/截图）与键盘等价：
`↑`/`←`/`k`/`h` 上、`↓`/`→`/`j`/`l` 下、回车确定、`S` 存一张 `sim-<n>.png`
（整数倍放大的 8 位灰度 PNG）。窗口文字用 `examples/sim/assets/fonts/` 里的
Noto Sans CJK SC。
无头模式注入 `Autoplay` 走 `--steps` 拍（每拍 500 ms）后存一张截图，适合没有
窗口的机器上调试。

`examples/sim/app.rs` 与 `examples/sim/assets/` 是 `../ili9341-lcd-kkui/src/` 里
应用描述表与字体图标的副本（一份只依赖 `kk_ui` 与 `core` 的表，两边各带一份，
换字体或改 Demo 时记得同步）。

## 许可证

跟随上游 KK_UI，MIT。字体与图标资源来自上游标准 Demo
