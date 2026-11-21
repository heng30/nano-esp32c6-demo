# AGENTS.md

本仓是 **ESP32-C6 的 Rust 裸机示例集合**，一个 **cargo workspace**：根 `Cargo.toml` 统一管理版本、
特性、profile 与工具链，每个子目录是一个 member crate（各自 `.cargo/config.toml`，因为 runner 和
`rustflags` 各不相同）。所有示例同一技术栈：`esp-hal 1.2` + `esp-rtos` + `embassy`，
目标三元组 `riscv32imac-unknown-none-elf`，日志一律 `defmt` over RTT。

本文件随当前工作目录加载；改哪个子目录的例子就先看对应 `README.md` 的接线表。

## 仓库地图

| 目录 | 内容 | 注意 |
|---|---|---|
| `http-ping/` | 本目录：起 WiFi AP（`esp32c6` / `12345678` / 网关 `192.168.2.1`），80 端口响应 `GET /ping` 回 `pong` 并闪 RGB LED（GPIO8），内置 DHCP 服务端 | `curl http://192.168.2.1/ping` 验证；本 crate 自带 `#[panic_handler]` |
| `3461BS/` | 4 位数码管动态扫描（GPIO0–4 段选、GPIO10/11/18/19 位选） | |
| `ILI9341-LCD/` | 240×320 屏，16 位 8080 并口 + `mipidsi`/`embedded-graphics`，占 18 根 GPIO | |
| `qapass-1602a-lcd/` | 1602A 字符屏，HD44780 4 位并行 | |
| `rainbow/` | 板载 WS2812B（GPIO8）彩虹循环，`esp-hal-smartled` + RMT，无 embassy | `DEFMT_LOG="off"`，不用 `rtt-target`；`#[panic_handler]` 自己实现 |
| `ble-server/` | BLE 示例，`trouble-host` + `esp-radio`；二进制名 `server`（`[[bin]]` 指定） | `.cargo/config.toml` 里多一条 `build-std = ["core"]`；链接参数直接写在 `rustflags`，**不用 `build.rs`** |
| `ble-led/` | BLE 控制板载 WS2812B：SIG `0x1815`/`0x2BE2` 特性写 ASCII `on`/`off`，`trouble-host` + `esp-hal-smartled` | 同上：`build-std` + `rustflags` 给链接参数，无 `build.rs`；接线见 `ble-led/README.md` |
| `esp32c6-top50/` | 抓下来做参考的上游仓库克隆 + 报告（`REPORT.md`、`BARE-CHIP-ONLY.md`） | **只读**，已 gitignore，不要改、不要当本仓代码提交 |
| `openc6-bios-rs/` | OpenC6 BIOS 微内核（从上游 ESP-IDF C 项目移植）：U-Mode PMP 沙箱 + `ecall` 系统调用、日志式环形 flash 文件系统、ZC6 压缩、Micro UNIX Shell、Wi-Fi AP/STA + BIOS Setup Web UI、PXE/OTA | 自带全局汇编 trap 向量表（`.rwtext`，必须 IRAM）；`src/web/index.html` 直接来自上游；`Rompass__openc6-bios` 是指向 `esp32c6-top50/` 的只读符号链接 |

## 依赖约定（workspace）

- **版本只在根 `Cargo.toml` 出现**，成员里一律 `xx.workspace = true`，不写版本号；
  根里版本号只保留两位（`"1.2"` 而非 `"1.2.2"`），唯一例外是 `esp-radio` 这种只有预发布的
  （`"1.0.0-beta.1"` 写全）。
- **全部依赖 `default-features = false`**，特性按实际用到的最小集合显式列出。
- 绝大多数依赖各成员需要同一套特性，直接写根里；**只有 `esp-hal` / `esp-rtos` / `esp-radio` /
  `esp-alloc` 允许成员用 `features = [...]` 追加**——它们的并集会把 WiFi+BLE 整套塞进纯显示的
  例程，还必须带 `esp-alloc` 全局分配器。新增成员时先看它到底要不要 `esp-radio`。
- `version` / `edition` 由根的 `[workspace.package]` 提供，成员写 `version.workspace = true`、
  `edition.workspace = true`。
- `[profile.*]` 只在根生效，成员里写会被忽略（cargo 会警告）；改编译选项去根 `Cargo.toml`。
- 工具链只在根 `rust-toolchain.toml` 声明（`channel = "stable"`，当前落到 `1.100.0`），
  成员目录不再各自一份。
- 依赖升级：改根 `Cargo.toml` 一处 → 根 `Cargo.lock` 统一刷新 → 六个目录逐个 `cargo build`。
  `esp-hal` 一档升级会连带 `trouble-host`（`server.rs`/`headset.rs` 的 `Stack`/`AttributeServer`
  API 变动很频繁，升级后先 `cargo build` 这两个 crate）。

## 本地环境（踩过的坑）

- **`cargo run` 就是烧录 + 跑**：`.cargo/config.toml` 里 runner 是 `probe-rs run --chip=esp32c6`，
  需要 USB 调试探针插着。探针是独占的——另一个终端里的 `probe-rs` / `cargo run` 还在跑就先停掉，
  否则报占用或抢到一半。
- **日志是 RTT，不是串口**：`defmt` + `rtt-target`，`main` 开头必须 `rtt_target::rtt_init_defmt!();`。
  串口工具看不到任何东西，别去 `minicom` 里找日志。
- **`DEFMT_LOG` 必须显式设置**：`defmt` 1.x 在环境变量缺失时只保留 error 级别，
  `info!` 会被编译成空操作。已在 `.cargo/config.toml` 的 `[env]` 里写死 `DEFMT_LOG = "info"`，
  改日志级别改这里，而不是在代码里加 `if`。
- **release 下要 `debug = 2` 才有日志位置**：否则 defmt 输出 `<invalid location: defmt frame-index: N>`。
  根 `[profile.release]` 已带 `debug = 2`，不要删。
- **链接脚本顺序**：`rustflags` 里 `-Tdefmt.x` 必须在 `-Tlinkall.x` **之前**，`linkall.x` 最后。
- **不要用 `build.rs` 传链接参数**：一律写在各目录 `.cargo/config.toml` 的 `rustflags`（`ble-server`、`ble-led` 已按此改）。
- **`#[panic_handler]` 只能有一份**：`http-ping` 自己实现了（`defmt::error!` + `software_reset`），
  再加 `panic-rtt-target` 会重复定义符号，链接失败。
- 入口两种写法：`http-ping` 用 `#[esp_hal::main]` + 手动 `esp_rtos::start(timg0.timer0, ...)`；
  其它示例用 `#[esp_rtos::main]`。两者不通用，照抄时连同一起来。
- `esp_rtos::start` 的第二个参数现在是 `peripherals.FROM_CPU_INTR0`；`SoftwareInterruptControl`
  和 `SW_INTERRUPT` 在 `esp-hal 1.2` 已删除。
- `defmt` 是公共特性，`esp-hal`/`esp-rtos`/`esp-radio` 在根上统一开着，所有示例都能打出栈内日志。
- BLE 两个 crate 的 `esp-alloc` 必须带 `compat`（给 WiFi/BLE 的 C 代码提供 `malloc`/`free`），
  去掉会出现 `undefined symbol: malloc_internal` 链接错误。

## 硬件工具（`hardware-*` / `gpio_*`）

日常开发仍用 `cargo run`；下面的工具用于**在没有打开代码编辑器流程时**摸板子状态、查手册、单独烧镜像。
它们会真改硬件状态，动作前先想清楚。

| 工具 | 什么时候用 |
|---|---|
| `hardware_devices` | 接板子后第一步：看有没有串口、VID:PID、配置里的设备别名 |
| `hardware_board_info` | 确认探针后面是哪颗芯片、架构、内存图（有探针时实测，否则回退内置表） |
| `hardware_memory_map` | 要 flash/RAM 地址区间时（DMA 缓冲区、`StaticCell` 位置排查） |
| `hardware_memory_read` | 读实时内存/寄存器，hex dump，单次 ≤256 字节；查外设寄存器是否被正确配置 |
| `hardware_flash` | 单独烧 `.elf`/`.hex`/`.uf2`/`.bin`；**ESP 用 ESP-IDF bootloader 布局时要 `format: "idf"`**，裸 `.bin` 必须给 `address`。日常改动别用它，`cargo run` 更省事 |
| `hardware_search` | 查本地数据手册（ESP32-C6 手册与 nanoESP32-C6 板图在 `datasheets/nano-esp32c6/`）：引脚复用、电气参数、外设寄存器、errata |
| `hardware_datasheet_import` | 新器件手册 PDF 归位到 `datasheets/<板/芯片>/`，自动切章节 MD 并重建 `overview.md`；之后用 `hardware_search` 查 |
| `hardware_datasheet_tidy` | 导入后的章节 MD 混着页眉页脚/水印时清一遍 |
| `gpio_read` | 读某个 GPIO 电平（板子号码），排查接线/上下拉/输出是否生效 |
| `device_*`、`arduino_upload`、`gpio_rpi_read` | 与本仓无关（MicroPython / Arduino / 树莓派），别对 ESP32-C6 的 Rust 示例用 |

查器件参数**先 `hardware_search`**，不要去猜、也不要翻 `esp32c6-top50/` 里某个素未谋面的仓库当权威。

## 代码约定

- `#![no_std]` + `#![no_main]`，`edition = "2024"`；有 `alloc` 需求先 `extern crate alloc;` 并
  `esp_alloc::heap_allocator!(size: ...)`。
- **注释用中文，代码里的字符串一律英文**：`defmt` 日志、`panic!`/`expect` 的错误信息都用英文，
  只有注释和 `///` 文档注释用中文——中文字符串在 RTT 里只安字节打印，排查时还得反查源码。
- `struct`/`enum` 及其字段与变体、`const`/`static`、`fn` 都要写 `///`，
  字段注释写“是什么 + 单位/取值”；不复述名字、不给显然的代码加废话注释。
- `const` 抽出来放文件顶部（SSID、端口、超时这类），别散在函数里。
- 定时/超时常量用 `Duration`；`Rate::from_mhz` 之类不带裸数字。
- **改引脚必须同步改该目录 `README.md` 的接线表**：现有 README 都有一张「接线」表格，
  这是本仓唯一的硬件文档真源，代码里不重复列引脚。
- 新增器件：手册 PDF 用 `hardware_datasheet_import` 归位，接线表写进该目录 README，再写代码。

## 构建 / 检查

```bash
cd http-ping          # 必须 cd 进子目录：runner/rustflags 来自各目录的 .cargo/config.toml
cargo check           # 交叉检查，不碰板子
cargo build --release # lto = "fat"，慢
cargo run             # 构建 + 烧录 + RTT 日志（需探针）
cargo clippy
cargo fmt             # 仓内无 rustfmt.toml、无 git hook，格式靠自觉
```

- **`cargo check --workspace` / `cargo build --workspace` 会失败**：仓根没有 `[build] target`，
  默认按宿主编译；即使加了 `--target`，workspace 级构建会把所有成员的特性取并集，显示例程被
  强行拉进 WiFi+BLE 并集，报 `no global memory allocator found`。**一个一个 crate 编**。
- 产物统一在仓根 `target/`（`/target` 已在 `.gitignore`），各成员不再有自己的 `target/`。

`make` 等任务本仓没有；CI 也没有，全靠本地。

## 提交

- 中文提交信息，逐行前缀：新增 `[+]`、删除 `[-]`、修改 `[*]`，行间不留空行，顺序 新增 > 删除 > 修改。
- 用显式路径 `git add <文件>`，**不要** `git add -A` / `git add .`（`target/`、`esp32c6-top50/` 会被误收）。
- 纯格式化（`cargo fmt`）与逻辑改动分开提交。
- 没被要求就别自动提交。
