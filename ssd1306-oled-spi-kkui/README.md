# ssd1306-oled-spi-kkui

把 `kk-ui-rs`（[KK_UI](https://gitee.com/keysking/kk_ui) 的 Rust 移植）的**模拟例子**
（`kk-ui-rs/examples/sim`）原样移植到 0.96 寸 SSD1306 128×64 OLED 上：首页图标选择器、
设置菜单、组件演示、数据监测、关于设备、波形自绘页、数学自绘页，以及整数/开关编辑器、
确认框、消息框与 Toast。

- 栈：`esp-hal 1.2` + `esp-rtos` + `embassy`（`#[esp_rtos::main]`）
- 显示：`ssd1306 0.10`（crates.io，只开 `BasicMode`，horizontal 寻址整帧一次 `draw`）+ `embedded-hal-bus` 的 `ExclusiveDevice` 按事务翻转 CS，SPI 8 MHz，只用 5 根 GPIO
- UI：`kk-ui-rs 0.3`（crates.io，只依赖 `core`，开 `cjk-fonts` 内嵌 16×16 中文点阵）。
  画布就是屏的 GDDRAM：128×64 的 1 bit 帧缓冲（1024 字节）与 SSD1306 的页式布局逐字节一致，
  所以一帧就是一次连续 SPI 写，**不放大、不做位序转换**
- 菜单行高 16 px，128×64 一屏 4 行；弹窗与首页图标组按画布高度垂直居中
- 刷新：只在 UI 报告 `take_frame_ready()` 时整帧递交，全屏一次约 1 ms（1024 字节 @8 MHz），
  日志每 5 秒打印一次 `flush=` 平均值
- 日志：`defmt` over RTT（不是串口）

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
- 屏是 3.3 V 器件，C6 的 GPIO 也是 3.3 V，不需要电平转换。
- 时钟 8 MHz，低于 SSD1306 的 10 MHz 上限；接线长或排线质量差时可把 `SPI_FREQ_MHZ` 调低。

## 按键

板载 **BOOT** 键（GPIO9，内部弱上拉，按下为低）：

| 操作 | 含义 |
|------|------|
| 短按（不足 700 ms） | `确定` |
| 按住超过 700 ms | `向下` |

没有第三个键：没人操作时 `app::Autoplay` 每 500 ms 按当前界面状态注入一次按键
（`src/app/autoplay.rs`，会顺带送编码器增量来翻位掩码），界面自己走遍各页与各类弹窗。
连续 15 秒界面状态不变就重置自动演示，避免某个弹窗把它卡死。

## 运行

```bash
cargo run --release
```

RTT 日志正常时是这样：

```text
[INFO ] ssd1306 ready 128x64
[INFO ] kk_ui init: ok canvas=128x64
[INFO ] first frame flushed
[INFO ] autoplay page=1 sel=0 dialog=false key=0 enc=0
[INFO ] t=5003ms page=2 sel=1 dialog=false still=480 frames=41 flush=1us
```

`page=` 是 `APP.routes` 里的下标 + 1（1 首页、2 设置、3 数据监测、4 波形、5 关于设备、
6 组件演示、7 数学）。

## 说明

- `src/app.rs` 与 `src/app/` 是 `kk-ui-rs 0.3` 自带 `examples/sim/app/` 的副本（图标在
  `src/assets/`），只改了「屏幕尺寸」文案（`128x64`）。上游改 Demo 时要同步这里与
  `ili9341-lcd-kkui/`。
- 亮度控制：SSD1306 有 `0x81` 对比度命令，但 UI 的亮度绑定约定是「屏不支持调光就上报
  不支持」，本示例沿用这条路径——第一次 `process_events` 后「屏幕亮度」条目会被置灰并
  弹一次 Toast，其余条目正常可用。
- 屏幕手册：驱动 IC 的 `SSD1306` Rev 1.1、同规格 7 针 128×64 模块规格书
  （`WEO012864ZWPP3N00000`）与 Waveshare 0.96" OLED 用户手册都已导入本地数据手册库
  （`hardware_search` 可查命令表、GDDRAM 布局与引脚/时序），原件另有一份在 `doc/`。
  卖家的 `WGSD E6045A` 是店铺 SKU、没有公开手册，引脚与命令以这三份为准。
- 画布高度必须是 8 的倍数：帧缓冲按 `width * ceil(height / 8)` 布局，`flush` 里直接
  按整块递交，非 8 倍数会错位。
