# ssd1306-oled-spi-mirui

用 [mirui](https://mirui.rs) 0.57 在 0.96 寸 SSD1306 128×64 OLED（SPI）上画一个**音乐播放器页**：
左边自绘转盘（纹路环 + 随 33⅓ rpm 转动的标记线 + 沿纹路向外爬的唱针与唱臂），
右边是曲名、进度条和上下三行歌词（当前行反白）。

- 栈：`esp-hal 1.2` + `mirui 0.57`（`default-features = false`，只依赖 `core`/`alloc`）。
  没有 async 也没有 RTOS：`App::run()` 本身就是个阻塞循环，入口直接用 `#[esp_hal::main]`。
- 渲染：mirui 的 `FramebufSurface` 拿到一块 128×64 的 RGB565 帧缓冲（16 KB，从堆上出），
  每帧在 flush 回调里按 Rec.601 亮度阈值压成 1 bit、按页式布局重排成 1024 字节，
  再整块写进 GDDRAM。所以**屏上只有黑白**，除黑/白以外的颜色一律按亮度取舍。
- 时钟：mirui 在 `no_std` 上不内置时钟源，`src/main.rs` 的 `EspClock` 插件把
  `esp_hal::time::Instant` 接给 `mirui::core::time::set_clock`，并插入 `MonoClock` 资源。
- 播放状态：`Signal<Readout>` 存曲名与三行歌词，`playback_tick` 系统每帧按 `MonoClock`
  重算并 `set` 一次；`ui!` 里五个 `${ ... }` 绑定跟着重绘。转盘的角度与唱针位置不经过
  ECS，直接在 view 的 paint 里现算，系统里一句 `for _deck in deck {}` 只是把它标脏。
- 播放列表 3 首共 36 秒循环（与 `ili9341-lcd-lvgl` 的音乐页同一套时长与歌词节奏）。

## 接线

7 针 SPI 模块（丝印可能是 `GND VCC D0 D1 RES DC CS`，也可能是 `GND VCC SCL SDA RES DC CS`）：

| 屏丝印 | C6 | 说明 |
|--------|----|------|
| GND | GND | |
| VCC | 3V3 | 3.3 V，**不要接 5 V** |
| D0 / SCL / CLK | GPIO6 | SPI 时钟（SCLK） |
| D1 / SDA / MOSI | GPIO7 | SPI 数据（MOSI） |
| RES | GPIO2 | 复位，低有效 |
| DC | GPIO1 | 数据/命令选择，低=命令 |
| CS | GPIO0 | 片选，低有效 |

- 4 针 I2C 模块（只有 GND/VCC/SCL/SDA）**不能用**：本示例走 SPI，模块背面需要能把
  DC 引出来的 7 针版本。
- 时钟 8 MHz，低于 SSD1306 的 10 MHz 上限；接线长或排线质量差时可把 `SPI_FREQ_MHZ` 调低。

## 运行

```bash
cargo run --release
```

RTT 日志正常时是这样：

```text
[INFO ] ssd1306 ready 128x64
[INFO ] mirui ready 128x64
[INFO ] first frame pushed              ← 第一帧已经推到屏上
[INFO ] t=5000ms frames=141 push=1300us ← 每 5 秒：这段刷了多少帧、每次整帧递交多久
```

`push=` 基本就是 1024 字节 @8 MHz 的传输时间加上 RGB565 → 1 bit 的换算；
帧率上限由 mirui 的软件光栅器 + 一次整屏 SPI 写决定，实测 20–30 fps 量级。

## 说明

- 屏只有 128×64 且是 1 bit，配色只用黑白两色：`OnSurfaceVariant` 这类中间灰按亮度落到
  黑色一侧，会直接看不见，所以歌词三行都用 `OnSurface`，靠当前行反白区分。
- 曲名按 `font_size: 8` 只放得下 8 个字符，歌词按 `font_size: 6` 一行十来个字符；
  改文案要先看这一列 66 px 的宽度。
- `ProgressBar` 的轨道色 `SurfaceVariant` 在 1 bit 屏上是黑，只有填充段看得见。
- 转盘盘面刻意只画白线、盘体留黑：唱臂也是白的，盘体填白的话唱臂会整段消失。
- **没有按键**：这块屏只有 128×64，摆不下 `ili9341-lcd-lvgl` 那三个播放按钮，页子是自动播放的。
- mirui 的帧缓冲在堆上，`HEAP_SIZE = 160 KB`（`esp_alloc::heap_allocator!`）；
  `.bss` 约 163 KB、栈由链接脚本给到 276 KB，都在 512 KB SRAM 以内。
- 自定义控件走 `#[view(component = Turntable, ...)]`：`ui!` 里写 `Turntable (width: ..)`，
  `main.rs` 里必须 `app.with_widget(music::turntable_view())` 注册，否则这个控件不会画。
