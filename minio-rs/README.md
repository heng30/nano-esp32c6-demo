# minio-rs

Minino OS（[ElectronicCats/Minino](../esp32c6-top50/repos/ElectronicCats__Minino/)）的 Rust 移植，
**可行子集**：所有 esp-radio/trouble-host 栈能力范围内的功能完整实现；
被闭源 blob 挡死的功能（WiFi deauth、promiscuous 嗅探、SDR）做成显式 stub 菜单项，不假装修好了。
完整对照清单见 [docs/PORTING.md](docs/PORTING.md)。

## 接线

nanoESP32-C6 上已接好的：

| 器件 | 引脚 | 说明 |
|---|---|---|
| 板载 RGB WS2812B | GPIO8 | 状态灯（隐身模式下熄灭） |
| 板载 BOOT 键 | GPIO9 | 菜单唯一物理输入，低电平按下 |

计划外接（**未接线，属计划值，接线后如改线必须同步改本表**）：

| 器件 | 引脚 | 说明 |
|---|---|---|
| SSD1306 OLED 128×64 (I2C) | SDA=GPIO19、SCL=GPIO18、VCC=3V3、GND | 菜单显示 |
| GPS 模块 (UART 9600) | GPS TX → GPIO17、VCC=3V3、GND | wardriving 定位 |
| microSD (SPI 模式) | CS=GPIO4、SCK=GPIO3、MISO=GPIO2、MOSI=GPIO1、VCC=3V3 | pcap/CSV 落盘 |

## 两种固件形态

esp-radio 在编译期就禁止 802.15.4 与 WiFi 同二进制（build.rs 强制），同上游 Minino 的 SDR 变体先例，分两种固件烧录：

| 形态 | 构建 | 可用 | 不可用 |
|---|---|---|---|
| WiFi（默认） | `cargo build` | portal / wardriving / settings 全部 | 802.15.4 四个菜单项 |
| 802.15.4 | `cargo build --no-default-features --features ieee154-build` | 嗅探 / 欺骗 / Thread 广播 | portal / wardriving |

跨形态菜单项进入时会提示需要哪种固件。另：esp-radio 的 `coex` 特性强制要求 ble+wifi 同时开启（build.rs 断言），所以 wifi 形态携带 ble 特性。

## 构建 / 运行

```bash
cd minio-rs
cargo check
cargo run   # 烧录 + RTT 日志，需探针
```

**验证 NV 持久化不能靠 `cargo run`**：probe-rs 烧录会把 `nvs` 分区一起擦掉，每次都从
`settings: defaults` 开始。要验证真实持久化，烧一次后再免烧录复位：

```bash
cargo run                       # 先烧入固件
probe-rs reset --chip=esp32c6   # 复位，不烧录
probe-rs attach --chip=esp32c6 target/riscv32imac-unknown-none-elf/debug/minio-rs
# 应看到 settings: loaded from nvs
```

依赖 `esp-radio` 带 `wifi` + `ble` + `coex`（成员特性追加），`ieee802154` 在 802.15.4
嗅探任务落地时再开。
