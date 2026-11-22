# ili9341-lcd

ESP32-C6 直接驱动野火 2.8 寸 ILI9341 电阻屏（240×320，16 位 8080 并口），不用任何外部芯片。

16 根数据线 + RS(DC) + WR，共 18 根 GPIO。CS 接地、RD 接 3V3、LCD_BK 接 GND，RST 接板子的 RST 排针。

- 栈：`esp-hal 1.2.2` + `esp-rtos` + `embassy`（`#[esp_rtos::main]`）
- 显示：`ili9341 0.6`（`graphics` 特性）+ `embedded-graphics 0.8`；初始化、地址窗口、`DrawTarget` 全部交给库，本目录只实现 `display-interface` 的 `WriteOnlyDataCommand`（8080-16 并口没有通用外设，只能 GPIO 打时序）
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
| **RS** / DC | GPIO15 | 寄存器选择，低=命令，高=数据。数据手册里叫 D/CX |
| WR | GPIO23 | 写脉冲，上升沿锁存 |
| RST | 板子 RST 排针 | 与 C6 的 EN 共复位；也可拉 3V3 靠上电复位 |

### 屏侧固定电平

| 屏丝印 | 接到 | 说明 |
|--------|------|------|
| CS | GND | 常选中 |
| RD | 3V3 | 只写不读（RD 低有效） |
| LCD_BK / BL | **GND** | 背光**低电平点亮**，接 3V3 会黑屏 |
| GND / VCC | GND / 3V3 | |

## 引脚为什么这么选

nanoESP32-C6 实际引出的排针只有这些：

| 排针 | GPIO |
|------|------|
| 上排 | 5V, 13, 12, 11, 10, 8, 1, 0, 7, 6, 5, 4, RST, 3V3, GND |
| 下排 | 5V, GND, 9, 18, 19, 20, 21, 22, 23, 15, RX, TX, 3, 2, GND |

**GPIO14 和 GPIO24–GPIO30 没有引出**，C6 芯片有、板子上没有，不能照着芯片手册排。

被排除的引脚：

| 引脚 | 原因 |
|------|------|
| GPIO9 | BOOT 键，内部弱上拉；按下会被拉低进下载模式，作数据线有短路风险 |
| GPIO12 / GPIO13 | USB-JTAG，`probe-rs` 烧录和 RTT 都走它 |
| GPIO16 (RX) | 接 CH343 的 TXD，被外部驱动，会打架 |
| GPIO17 (TX) | CH343 的 RXD 是高阻，能用，但串口会吐乱码，所以也不占用 |

16 位数据线加 DC/WR 要 18 根，干净引脚只剩 14 根，只能借两个：

- **GPIO8**：板载 WS2812 RGB LED 的数据脚，作数据线时灯会乱闪，功能无影响。
- **GPIO15**：JTAG 源选择 strapping，无内部上下拉。默认 eFuse 下 JTAG 走 USB Serial/JTAG，这个脚被忽略，浮空无碍；追求稳妥可加 10K 下拉到 GND。

## 运行

```bash
cargo run --release
```

`release` 是必须的：dev 构建慢一到多个数量级，8080 写时序会撑不住。

RTT 日志用：

```bash
probe-rs attach target/riscv32imac-unknown-none-elf/release/ili9341-lcd --chip esp32c6
```

## 注意

- 每个像素要逐根翻 16 次 GPIO，全屏大约 50–100 ms，十几 fps。静态画面够用，动画别想。
- 想提速的话，把 `Bus::write` 换成直接写 GPIO 的 `OUT_W1TS` / `OUT_W1TC` 寄存器，一次掩码写就能改完 16 根，能快十倍以上。
- 方向不对（画面镜像）就把 `Orientation::Landscape` 换成 `LandscapeFlipped`，别去改 MADCTL 常数。
- XPT2046 触摸部分没有实现，需要另外 4 根 SPI 线。
