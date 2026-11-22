# ili9341-lcd-slint

同一块野火 2.8 寸 ILI9341（240×320，16 位 8080 并口）上跑 **Slint 界面**，而不是手写像素动画。
屏幕接线与 `../ili9341-lcd/` 完全相同，**接线表以 [../ili9341-lcd/README.md](../ili9341-lcd/README.md) 为准**，
这里不重复维护第二份。

- 栈：`esp-hal 1.2` + `esp-rtos` + `embassy`（`#[esp_rtos::main]`）
- 界面：`slint 1.19`（`renderer-software` + `unsafe-single-threaded` + `libm` + `compat-1-2`）
- 堆：`esp-alloc`，64 KB（Slint 的组件树、字符串、字形缓存都在堆上）
- 显示：`ili9341 0.6` 只管初始化与地址窗口，8080 并口时序仍由本仓实现（同 `../ili9341-lcd/src/display.rs`）

## 文件

| 文件 | 职责 |
|------|------|
| `src/display.rs` | 18 根 GPIO 的并口时序 + 屏初始化（从 `../ili9341-lcd/` 复制） |
| `src/platform.rs` | `slint::platform::Platform` 实现（窗口 + 时间源）、`LineBufferProvider`（逐行落屏） |
| `src/main.rs` | 外设初始化、创建窗口、事件循环 |
| `ui/main.slint` | 界面本体 |
| `build.rs` | 构建期编译 `.slint`，把字体和资源嵌进固件 |

## 画面

8 页轮播，前 7 页各停 8 秒、最后的音乐播放器页停 40 秒（走完一遍播放列表），调度在 `src/effects/mod.rs`。
除第 0 页外，每个效果实现 `Canvas::draw(&mut [Rgb565Pixel], t_ms)`，**一个效果一个文件**：

| 页 | 文件 | 内容 |
|---|---|---|
| 0 | `ui/effects/demo.slint` | 移植自 ESP32-C3 示例：按钮 + 两个圆，`states` + `animate` 滑动 |
| 1 | `src/effects/geometry.rs` | 旋转三角形 + 中心固定的圆 |
| 2 | `src/effects/particles.rs` | 32 个粒子，重力、边界反弹、整屏衰减拖尾 |
| 3 | `src/effects/tunnel.rs` | 伪 3D 隧道：径向环 + 八向扇区 |
| 4 | `src/effects/bars.rs` | 16 柱动态频谱，柱高平滑 + 峰值保持 |
| 5 | `src/effects/clock.rs` | 模拟时钟（三针 + 刻度）+ 3×5 点阵数字时间 |
| 6 | `src/effects/plasma.rs` | 逐像素等离子波，三角波代替 sin |
| 7 | `src/effects/music.rs` + `ui/effects/music.slint` | 音乐播放器：黑胶旋转（标签与高光随盘转）+ 唱针由盘心移向盘边 + 进度条 + 上一首/暂停/下一首图标；歌词是 Slint `Text`，`animate y` 滚动并高亮当前行 |

除第 0 页外，画面统一是「Rust 逐像素画进 RGB565 缓冲 → `Image` → Slint 拉伸到全屏」。
第 7 页在此基础上多一层：画布的右侧（画布 x ≥ 88）留空，由 `MusicPage` 叠歌词文字——
`Canvas::sync()` 每帧只把当前歌词行号推过去，整个列表只在第一次播到该页时设一次。

屏没有触摸（XPT2046 那套没接线），所以上一首/暂停/下一首三个图标只是画上去的样子，
播放列表按时间轴自己走，进度条与唱针位置都由本首已播时长算出来。

## 渲染路径

```text
Slint 组件树
   ↓ window.draw_if_needed()
MinimalSoftwareWindow（软件渲染，只重画脏区）
   ↓ render_by_line()
platform::LineWriter::process_line()   ← 每行回调一次
   ↓ draw_raw_slice(x0, line, x1, line)
ILI9341 8080 并口
```

- 像素页的位图只有 **160×120**（76.8 KB），由 `.slint` 的 `image-fit: fill` 放大到 320×240。
  全屏 320×240 位图要 150 KB，逐帧分配直接把堆撑爆（实测 `allocation of 76800 bytes failed`）。
- 并口落屏**只有一行缓冲**（320 × RGB565 = 640 B），不存整帧。整帧 150 KB 在 512 KB SRAM 的 C6 上放不下，
  而且 `ReusedBuffer` 模式下屏上内容本来就不需要留在 RAM 里。
- 每行一次窗口设置 + 320 个像素写；实测首帧 240 行全部刷完，肉眼无撕裂。
- 换页时长按页区分（`PAGE_DURATION` / `MUSIC_PAGE_DURATION`），音乐播放器页要停得够久才看得出进度和歌词。
- `main` 里没有动画在跑时按 `duration_until_next_timer_update()` 睡，避免空转占满并口。

## 字体与中文

Slint 在裸机上没有系统字体，**必须在构建期嵌入 TTF**，所以仓库根 `../assets/DejaVuSans.ttf`
是构建必需资源（来源与许可见 `../assets/README.md`）。

- 该字体只覆盖拉丁/希腊/西里尔，**没有汉字**。
- 要在界面里放中文，不能走 `Text` 元素，得把汉字点阵（`../ili9341-lcd/` 的 GB2312 字模那套）
  画进 `SharedPixelBuffer`，再用 `Image { source: ... }` 贴上去——中文此时是图片，不能参与文字布局、
  自动换行或行内混排。
- 换 `font-size` 会改变构建期生成的位图字形数量，从而影响固件大小；一个字号一套字形。

## 授权

Slint 是 **GPLv3 或商业授权**双许可，嵌入式产品要商用需买授权。本目录作为开源示例按 GPLv3 使用；
不要把这个 crate 直接塞进闭源固件。

## 运行

```bash
cargo run --release
```

`release` 是必须的：软件渲染 + 并口写时序在 dev 构建下撑不住。

RTT 日志：

```bash
probe-rs attach target/riscv32imac-unknown-none-elf/release/ili9341-lcd-slint --chip esp32c6
```

## 注意

- `esp_alloc::heap_allocator!` 只能写在函数体内，写成模块级 item 会报
  `macro expansion ignores '{'`。
- `slint::platform::set_platform` 只能成功调用一次，窗口在 `platform::init()` 里建好并克隆出来。
- 界面不能每帧全量重绘：满屏裸写像素 28.8 ms 是硬下限，务必靠脏区刷新。
- 该 crate 的 `Cargo.toml` 里 `slint-build` 是构建期 host 工具，保留默认特性（它需要 std）。
- **软件渲染器不支持 `Path` 和 `transform`**（`.slint` 编译直接报 Unknown element / Unknown property），
  所以任意线段、旋转都只能在 Rust 里逐像素画，Slint 侧只剩轴对齐矩形、文本和 `Image`。
- `Image::from_rgb565` 需要 `slint` 的 `image-pixel-format-rgb565` 特性，默认不开。
- 12 小时制的 `hour` 会取到 10、11，用点阵表索引前必须先转成 `HH` 两位数，否则越界 panic（已踩过）。
