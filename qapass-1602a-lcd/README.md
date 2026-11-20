# QAPASS 1602A LCD

字符型 16×2 液晶模块（QAPASS 1602A，HD44780 兼容控制器），ESP32-C6 + esp-hal/embassy 直接驱动。
上电第一行显示 `QAPASS 1602A`，第二行每秒刷新运行秒数。

- 栈：`esp-hal 1.2.2` + `esp-rtos` + `embassy`（`#[esp_rtos::main]`）
- 接口：4 位并行（RS + E + DB4~DB7），不用外部驱动芯片
- 日志：`defmt` over RTT（不是串口）

## 接线

模块是 16 个排针位置，全部按下面接。

| 1602A 脚 | 符号 | 接到 | 说明 |
|---|---|---|---|
| 1 | VSS | GND | 地 |
| 2 | VDD | 5V | 逻辑电源 4.5–5.5V，**不能接 3V3** |
| 3 | V0 | 10K 电位器中点 | 另一端接 5V / GND，调对比度；直接接地通常全黑或全亮 |
| 4 | RS | GPIO0 | 低=指令，高=数据 |
| 5 | R/W | GND | 只写，不读忙标志（省一根线，也避免 5V 输出打回 C6） |
| 6 | E | GPIO1 | 使能脉冲，上升沿有效，脉宽 ≥230ns |
| 7–10 | DB0~DB3 | 悬空 | 4 位模式下不用 |
| 11 | DB4 | GPIO2 | 数据高四位 |
| 12 | DB5 | GPIO3 | |
| 13 | DB6 | GPIO4 | |
| 14 | DB7 | GPIO5 | |
| 15 | LED+ | 5V 串 220Ω | 背光阳极，**必须串限流电阻** |
| 16 | LED- | GND | 背光阴极 |

## 电平

LCD 用 5V 供电，但输入高电平门限 VIH 最低 2.2V（手册 Electrical Characteristics），
C6 的 3.3V 输出足够，不需要电平转换。反向没问题：R/W 接地，模块不会向 GPIO 输出 5V。

## 运行

```bash
cargo run --release
```

RTT 日志：

```bash
probe-rs attach target/riscv32imac-unknown-none-elf/release/qapass-1602a-lcd --chip esp32c6
```

## 实现要点

- 初始化按手册 `Initializing of LCM`：上电等 50ms，连发三次 `0x3` 把控制器拉回 8 位状态，再发 `0x2` 切 4 位，
  之后 `0x28`（4 位 / 2 行 / 5×8 点阵）、`0x08` 关显示、`0x01` 清屏、`0x06` 地址自增、`0x0C` 开显示。
- 4 位模式下每个字节先发高四位再发低四位；RS 在两次 nibble 之间保持不变。
- 不用 BF（忙标志）判忙，改用固定延时：普通指令 50us、清屏/回车 2ms（手册标称 1.53ms）。
- 行地址：第一行 DDRAM `0x00`，第二行 `0x40`，写 `0x80 | addr` 定位。
- 实现 `core::fmt::Write` 后可直接 `write!(lcd, "uptime {:<9}", secs)`，尾部补空格盖掉上次残留字符。
