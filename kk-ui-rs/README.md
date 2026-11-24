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
  行高 16、菜单表头 18、信息页表头 18 仍是常量：屏越高菜单越多行，首页图标组与弹窗
  按画布高度垂直居中，Toast 贴底，弹窗与 Toast 横向居中，跨页动画的位移跟着宽度走。
- **只有一种字体尺寸**：16×16 定长点阵、顶部对齐，全角 16 px 步进、半角按表里的
  原始宽度；U8g2 的 RLE 字库格式与旋转/基线参考高度都没有移植。
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

应用描述表（`App`）是编译期常量：路由、页面、绑定与文案，结构对应 C 版
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

## 字体：全局缓存 + 内嵌点阵（`cjk-fonts`）

整个 UI 只有一个当前字体，放在库内全局缓存里，页面与弹窗不再各自持有：
`Frame::draw_text` / `Frame::text_width` 直接读它。设置函数在程序初始化时调用一次：

```rust
// 启用 cjk-fonts 特性时默认就是内嵌字库，初始化无需任何调用。
// 要换成自己的 16×16 字库：
kk_ui::set_font(&MY_GLYPHS);
// 要换回内嵌字库：
kk_ui::set_font(&kk_ui::cjk::GLYPHS);
```

字库是「点阵 / 宽度 / 码点」三张平行静态表，自己的表只需按同一布局提供：

```rust
pub struct Glyphs {
    pub bitmaps: &'static [u8],     // 每个字形 32 字节，16 行 × 2 字节，行内 MSB 在左
    pub widths: &'static [u8],      // 像素宽度，全角 16、半角按实际
    pub codepoints: &'static [u8],  // u16 小端升序，二分查找
}
```

不开 `cjk-fonts` 时初始未设置字体，文字绘制为空操作，须先 `set_font`。

`cjk-fonts` 的数据由 `build.rs` 在编译期从仓库自带的 `assets/fonts/unifont.bdf`
里抠出，写进 `OUT_DIR` 后嵌进库：GB2312 常用汉字、中文标点、全角字符与制表符，
以及全部 ASCII 大小写字母和常用英文符号，所以中英混排文本（`FPS: 80%`、
`返回Back`）可以直接画：

```bash
cargo build --features cjk-fonts
```

```rust
let mut frame = Frame::new(&mut buffer, 128, 64);
// Frame 的文字接口走全局字体，也可以直接查内嵌字库。
frame.draw_text(0, 0, "屏幕亮度：80% FPS: 30");

if let Some(bitmap) = kk_ui::cjk::GLYPHS.glyph('中') {
    // 16×16 点阵，行内 MSB 在左，每行 2 字节
}
```

字形固定 16×16：全角 16 px 步进，半角（ASCII 及全角区里的窄字）按 BDF 原始宽度。
码点二分查找，字库约 7170 个字形（95 个 ASCII + 6763 个 GB2312 汉字 + 中文标点、
全角字符、制表符），编进库约 245 KB。`assets/fonts/unifont.bdf` 是仓库内副本
（9 MB），没装 Unifont 的机器也能构建；不开 `cjk-fonts` 时这部分完全不参与编译。

## 资源工具 `kkui-tool`

包内唯一的可执行程序（`bin/kkui-tool.rs`），它依赖的 `clap` / `image` 归 `tool` 特性，
所以安装要带上特性：`cargo install --path . --features tool`（本地跑用
`cargo run --bin kkui-tool --features tool --`）。

```text
kkui-tool font <目录或文件>... [--out font]  # 采集字符，产出 3 张字库表 + text.txt
kkui-tool icon <图片> [--size 32] [-o 输出]   # png/jpeg/webp → XBM 位图（raw .bin）
```

`font` 的字符来源分两部分：**可打印 ASCII（英文字母、数字、英文标点）始终收录**；中文
则靠扫描——目录只认 `.rs` 文件（跳过 `target`/`.git`/`node_modules`），且只取字符串与
字符字面量里的字，**注释（含 `///` 文档注释）不算**。显式传入的非 `.rs` 文件按纯文本收全部
字符，方便把上一次生成的 `text.txt` 再喂回来。

字符按码点升序从 `assets/fonts/unifont.bdf` 里抠出字模，产出与 `build.rs` 同构的三张平行表，
`include_bytes!` 后即可组成 `Glyphs` 交给 `set_font`；同目录的 `text.txt` 是采集到的字符清单。
BDF 在编译期就 `include_bytes!` 进了工具（装出来的二进制约 21 MB），所以不需要传字库路径，
也不依赖 `cjk-fonts` 特性；能取到哪些码点由 BDF 决定，取不到的字符会被列出来但不入表。

`icon` 把图缩到 `--size` 见方再二值化（深色算点亮、透明算熄灭），按 `Frame::draw_xbm`
的位序写 raw 二进制：低位在前、每行 8 像素对齐，32×32 正好 128 字节。

本目录的 `Makefile` 把这两步串起来了：`make` 先给 `examples/sim` 生成字库再跑模拟器，
`make font` 只生成字库，`make snapshot` 出无头截图。

## 本地模拟器（宿主机）

`sim` 特性编译出一个小框架（`kk_ui::sim`，源码在 `src/sim.rs`），把 1 bit 画布显示到
eframe 窗口，并把窗口按键注入应用。库本体仍是 `#![no_std]`，不开这个特性时
`eframe` / `png` 都不会被拉进来。

```rust
kk_ui::sim::run(
    kk_ui::sim::Options { title: "demo", width: 160, height: 120, scale: 2 },
    move |ctx| {
        // 每帧调用一次：ctx.now 是毫秒时间戳，ctx.keys 是本帧按住的键
        ui.update(ctx.now, Input { keys: ctx.keys, encoder_delta: 0 });
        ctx.show(ui.frame()); // 交出这帧要显示的画布
    },
)?;
```

窗口固定大小、画布与按键栏都居中，底部一排大号按钮（上/下/确定/截图）与键盘等价：
`↑`/`←`/`k`/`h` 上、`↓`/`→`/`j`/`l` 下、回车确定、`S` 存一张 `sim-<n>.png`。
截图是 8 位灰度 PNG，`kk_ui::sim::save_png(frame, scale, path)` 也可以直接调，
无头调试时不必开窗口。

`examples/sim/` 是完整例子：`app.rs` 与 `assets/` 是 `../ili9341-lcd-kkui/src/` 里
应用描述表与图标的副本（改一边记得同步另一边）。无头模式
`cargo run --example sim --features "sim cjk-fonts" -- --snapshot out.png --steps 40`
让 Demo 自己走 40 拍再落盘。

## 许可证

跟随上游 KK_UI，MIT。图标资源来自上游标准 Demo
