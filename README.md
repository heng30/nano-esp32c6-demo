# nano-esp32c6-demos

nanoESP32-C6 开发板的 Rust 裸机（`no_std`）示例集合，一个 cargo workspace 管所有成员。
统一技术栈：`esp-hal 1.2` + `esp-rtos` + `embassy`，目标 `riscv32imac-unknown-none-elf`，
日志一律 `defmt` over RTT（**不是串口**）。

## 板子规格

nanoESP32-C6（MuseLab v1.0），模组为 `ESP32-C6-WROOM-1-N16`（芯片 rev v0.1，MAC `40:4c:ca:5f:ca:14`）：

| 项目 | 规格 |
|---|---|
| 核心 | 单核 32 位 RISC-V（HP），最高 160 MHz；另有 20 MHz LP 核 |
| SRAM | 512 KB HP + 16 KB LP |
| ROM | 320 KB |
| Flash | 16 MB Quad SPI（模组内的片外 flash，非片上） |
| PSRAM | 无 |
| 无线 | Wi-Fi 6（2.4 GHz）、BLE 5.3、802.15.4（Thread / Zigbee） |
| USB | 两个 Type-C：原生 USB-Serial-JTAG + CH343P（挂 UART0） |
| 供电 | USB-C 5 V，板载 AMS1117 稳到 3.3 V |
| 板载外设 | WS2812B RGB（GPIO8）、S1 = GPIO9（BOOT）、S2 = CHIP_PU（RST） |

512 KB SRAM 不是全给程序的：`esp-hal` 的 `ld/esp32c6/memory.x` 里 `RAM` 段只有 `0x6E610`（≈441 KB），
另有 64 KB 要等二级 bootloader 退出才释放，余下留给 flash cache。**没有 PSRAM**，几个字节以上的
缓冲只能放 `.bss`（`ili9341-lcd` 三个效果的帧缓冲合计约 157 KB），写新效果前先算总占用。

上表由 `espflash board-info` 实测（走 `/dev/ttyACM0` 的 UART 路，会复位板子进 flash stub）：
前者报 `Flash size: 16MB`，`probe-rs` 本身不报 flash 容量。

## 示例

| 目录 | 内容 | 接线文档 |
|---|---|---|
| `http-ping/` | 起 WiFi AP（`esp32c6` / `12345678` / `192.168.2.1`），`GET /ping` 回 `pong` 并闪板载 RGB，内置 DHCP 服务端 | crate 内常量为准 |
| `rainbow/` | 板载 WS2812B（GPIO8）彩虹循环，`esp-hal-smartled` + RMT，无 embassy | 见 `src/main.rs` 注释 |
| `3461BS/` | 0.36 吋 4 位共阳数码管动态扫描，每秒加一 | [链接](3461BS/README.md) |
| `1588BS/` | 1.5 吋 8×8 共阳点阵逐行扫描，循环播放全亮/棋盘/心形/叉号四帧 | [链接](1588BS/README.md) |
| `baike/` | 10 脚单个**共阴**数码管（A–G + DP + 公共端）静态驱动，段选 GPIO0–7、公共端接 GND，0–9 每秒加一 | [链接](baike/README.md) |
| `ili9341-lcd/` | 240×320 ILI9341 屏，16 位 8080 并口（18 根 GPIO），`ili9341` + `embedded-graphics`，六个动画效果 + GB2312 中文点阵 | [链接](ili9341-lcd/README.md) |
| `ili9341-lcd-slint/` | 同一块屏跑 `slint` 界面（软件渲染 + 逐行刷屏），需要 `esp-alloc` | [链接](ili9341-lcd-slint/README.md) |
| `ili9341-lcd-lvgl/` | 同一块屏跑 `lvgl 9.6`（`lvgl-rust` 安全包装）：`lv_image` 旋转唱片 + `lv_line` 唱臂 + `lv_bar` 进度 + `lv_label` 滚动歌词，需要交叉 C 工具链（`nix-shell`） | [链接](ili9341-lcd-lvgl/README.md) |
| `qapass-1602a-lcd/` | 1602A 字符屏，HD44780 4 位并行 | [链接](qapass-1602a-lcd/README.md) |
| `ble-server/` | BLE GATT server，`trouble-host` + `esp-radio`，二进制名 `server` | 无需外设 |
| `bilibili-follower/` | WiFi STA + 手写 HTTPS（`embedded-tls`），每分钟打印 B 站某 UP 主的粉丝数 | [链接](bilibili-follower/README.md) |
| `ble-led/` | BLE 控制的板载 RGB 灯：手机写 `on`/`off`，`trouble-host` + `esp-hal-smartled` | [链接](ble-led/README.md) |
| `minio-rs/` | Minino OS 的 Rust 移植：BOOT 键菜单（Portal / wardriving / 802.15.4）、SSD1306 菜单、microSD 落盘、NV 持久化；WiFi 与 802.15.4 是两种不能同固件的形态 | [链接](minio-rs/README.md) |
| `kkui-rs/` | KK_UI（128×64 单色菜单 UI）的 Rust 移植库 `kk_ui_rs/` + 在 ILI9341 上跑原版标准 Demo 的 `ili9341-kkui-demo/`；**独立 workspace**，依赖不继承根 `workspace.dependencies` | [链接](kkui-rs/ili9341-kkui-demo/README.md) |
| `openc6-bios-rs/` | OpenC6 BIOS 微内核（从上游 ESP-IDF C 项目移植）：U-Mode PMP 沙箱 + `ecall`、日志式环形 flash 文件系统、Micro UNIX Shell、Wi-Fi AP/STA + BIOS Setup Web UI、PXE/OTA | [链接](openc6-bios-rs/README.md) |

`esp32c6-top50/`（只读的上游仓库克隆与报告）已 exclude，不参与构建。

## 环境

- `rust-toolchain.toml` 声明的 stable + `rust-src` + `riscv32imac-unknown-none-elf`，`rustup` 会自动装
- `probe-rs` 及 ESP32-C6 调试探针（USB-JTAG）
- 引脚表以各目录 `README.md` 为准（nanoESP32-C6 实际只引出部分 GPIO）

## 构建与烧录

**必须 `cd` 进子目录操作**：各 crate 有自己的 `.cargo/config.toml`（runner、`rustflags`、`DEFMT_LOG` 各不相同），
产物统一落在仓根 `target/`。

```bash
cd rainbow
cargo check           # 交叉编译检查，不碰板子
cargo build --release # lto = "fat"，慢
cargo run             # 构建 + 烧录 + RTT 日志（需探针）
cargo clippy
cargo fmt
```

**不要用 `--workspace`**：仓根没有 `[build] target`，且 workspace 级构建会把所有成员的特性取并集，
纯显示例程会被强行拉进 WiFi+BLE 并集，报 `no global memory allocator found`。一个一个 crate 编。

探针是独占的，另一个终端里的 `cargo run` / `probe-rs` 还开着会占不到设备。

## 日志

- `defmt` + `rtt-target`，`main` 开头 `rtt_target::rtt_init_defmt!();`，串口工具里看不到任何东西
  （例外：`rainbow/` 不要日志，`DEFMT_LOG="off"`、不用 `rtt-target`、`#[panic_handler]` 自己实现）
- `DEFMT_LOG` 已写死在各自 `.cargo/config.toml` 的 `[env]`；`defmt` 1.x 缺省只保留 error，改级别改那里
- 已跑起来的固件可单独挂日志：`probe-rs attach target/riscv32imac-unknown-none-elf/release/<bin> --chip esp32c6`
- `release` 下需 `debug = 2` 才有日志位置，根 `[profile.release]` 已带，别删

## 依赖约定

版本只在根 `Cargo.toml` 的 `[workspace.dependencies]` 出现，成员一律 `xx.workspace = true`；
全部 `default-features = false`。只有 `esp-hal` / `esp-rtos` / `esp-radio` / `esp-alloc` 允许成员
用 `features` 追加——并集会把 WiFi+BLE 整套塞进纯显示例程。

升级依赖：改根 `Cargo.toml` → 根 `Cargo.lock` 刷新 → 逐个目录 `cargo build`。`esp-hal` 升级会连带
`ble-server` / `ble-led` 的 `trouble-host` API。

## 约定

- `#![no_std]` + `#![no_main]`，`edition = "2024"`
- 注释与文档用中文；`struct` / `enum` / `const` / `fn` 写 `///`
- **改引脚必须同步改该目录 `README.md` 的接线表**，那是本仓唯一的硬件文档真源
- 入口两种写法不通用：`http-ping` 用 `#[esp_hal::main]` + 手动 `esp_rtos::start`，
  其它用 `#[esp_rtos::main]`；`#[panic_handler]` 全仓每个 crate 只能有一份
- 提交用显式路径 `git add <文件>`，中文提交信息，行首 `[+]` / `[-]` / `[*]`
