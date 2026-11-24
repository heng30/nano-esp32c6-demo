# ili9341-kkui-demo

把 `kk-ui-rs`（[KK_UI](https://gitee.com/keysking/kk_ui) 的 Rust 移植）跑在野火 2.8 寸
ILI9341 屏上，内容是官方标准 Demo 的原样移植：首页图标选择器、设置菜单、组件演示、
数据监测、关于设备、波形自绘页，以及整数/开关编辑器、确认框、消息框与 Toast。

- 栈：`esp-hal 1.2` + `esp-rtos` + `embassy`（`#[esp_rtos::main]`）
- 显示：`ili9341 0.6`（`graphics` 特性）+ `embedded-graphics-core`，8080-16 位并口
  由本目录的 `display.rs` 打时序
- UI：`kk-ui-rs`（`../kk-ui-rs`，只依赖 `core`），画布尺寸由本例子给定：160×120 的
  1 bit 画布**放大 2 倍铺满** 320×240 的屏，黑底白字。菜单因此能显示 6 行，首页图标组
  与弹窗按画布高度垂直居中；换成别的画布只改 `CANVAS_WIDTH`/`CANVAS_HEIGHT`（行高仍 16 px）
- 刷新：`display::blit_canvas` 每条逻辑行展开成物理行后整行递交，全屏一次约 34 ms（≈29 fps
  上限），日志里每 5 秒打印一次 `blit=` 平均值
- 日志：`defmt` over RTT（不是串口）

## 接线

### 数据总线

| 屏 | C6 | 屏 | C6 |
|----|----|----|----|
| D0 | GPIO0 | D8 | GPIO8 |
| D1 | GPIO1 | D9 | GPIO10 |
| D2 | GPIO2 | D10 | GPIO11 |
| D3 | GPIO3 | D11 | GPIO18 |
| D4 | GPIO4 | D12 | GPIO19 |
| D5 | GPIO5 | D13 | GPIO20 |
| D6 | GPIO6 | D14 | GPIO21 |
| D7 | GPIO7 | D15 | GPIO22 |

数组下标和位序一一对应：`data[0]` = D0，`data[15]` = D15。

### 控制线

| 屏丝印 | C6 | 说明 |
|--------|----|------|
| **RS** / DC | GPIO15 | 寄存器选择，低=命令，高=数据 |
| WR | GPIO23 | 写脉冲，上升沿锁存 |
| RST | 板子 RST 排针 | 与 C6 的 EN 共复位；也可拉 3V3 靠上电复位 |

### 屏侧固定电平

| 屏丝印 | 接到 | 说明 |
|--------|------|------|
| CS | GND | 常选中 |
| RD | 3V3 | 只写不读（RD 低有效） |
| LCD_BK / BL | **GND** | 背光**低电平点亮**，接 3V3 会黑屏 |
| GND / VCC | GND / 3V3 | |

### 按键

板载 **BOOT** 键（GPIO9，内部弱上拉，按下为低）：

| 操作 | 含义 |
|------|------|
| 短按（不足 700 ms） | `确定` |
| 按住超过 700 ms | `向下` |

没有第三个键：另有一串每 2.5 s 注入一次的自动演示按键（见 `main.rs` 的
`AUTOPLAY_KEYS`），没人操作时界面也会自己走遍各页与各类弹窗。

## 运行

```bash
cargo run --release
```

`release` 是必须的：dev 构建慢一到多个数量级，8080 写时序会撑不住。

RTT 日志正常时是这样：

```text
[INFO ] ili9341 ready 320x240
[INFO ] kk_ui init: ok
[INFO ] first frame flushed
[INFO ] t=5004ms page=1 frames=19
```

`page=` 是当前页面编号（1 首页、2 设置、3 数据监测、4 波形、5 关于设备、6 组件演示），
自动演示会让它不断变化。

## 说明

- 本目录与 `../kk_ui_rs` 属于 `kkui-rs/` 这个**独立 workspace**：Cargo 依赖在这里
  各自钉版本，不用仓库根的 `workspace.dependencies`，产物落在 `kkui-rs/target/`。
- 亮度控制：C 版 Demo 会调 `OLED_SetContrast()`，本屏背光直接接 GND、没有可调光通路，
  所以照 UI 的约定如实上报「不支持」——第一次 `process_events` 后「屏幕亮度」条目会被
  置灰（带 ⊘ 标记）并弹一次 Toast。其余条目都是正常可用的。
- 「屏幕尺寸」一栏写 `128x64@2x`、「刷新方式」写 `Blocking`，与真实呈现方式一致。
- 8080 并口占掉 18 根 GPIO，其中 GPIO8 是板载 WS2812B 的数据脚（会乱闪，功能无影响），
  GPIO15 是 JTAG 源选择 strapping（默认 eFuse 下被忽略）。
