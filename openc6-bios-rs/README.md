# OpenC6 BIOS (Rust 移植)

把 [Rompass/openc6-bios](https://github.com/Rompass/openc6-bios)（ESP-IDF C 实现的 ESP32-C6 BIOS / 微内核）
移植到 Rust：`esp-hal 1.2` + `esp-rtos` + `embassy`，目标三元组 `riscv32imac-unknown-none-elf`，
日志 `defmt` over RTT。

上游源码以只读符号链接形式留在 `Rompass__openc6-bios/`，指向 `../esp32c6-top50/repos/Rompass__openc6-bios`，
移植时对照用，不参与构建。

## 接线

硬件按 ESP32-C6-Zero（或任意带 8MB Flash 的通用 C6 模组）接线。除 WS2812 外没有必须外接的器件，
其余引脚是板载按键 / 跳线 / USB。

| GPIO | 上游宏名 | 方向 | 说明 |
|---|---|---|---|
| 1 | `HAL_PIN_CMOS_GND` | 输入 | Clear CMOS 跳线一端，上电时对地短接即恢复出厂 NVRAM |
| 2 | `HAL_PIN_CLEAR_NVRAM` | 输入 | Clear CMOS 跳线另一端 |
| 3 | `HAL_PIN_PWR_BTN_GND` | 输出（0V） | 电源按键的虚拟地锁存 |
| 4 | `HAL_PIN_PWR_BTN_SENSE` | 输入（上拉） | 电源键 sense 线：唤醒 / 长按 3s 进 Setup / 长按 5s 强制复位 |
| 8 | `HAL_PIN_WS2812` | 输出 | 板载 WS2812B，POST 诊断与 Aura Sync |
| 9 | `HAL_PIN_BOOT_BTN` | 输入（上拉） | 物理 BOOT 键，上电时按住进 Boot Menu |
| 12 | `HAL_PIN_USB_DM` | — | USB-Serial-JTAG 原生 D- |
| 13 | `HAL_PIN_USB_DP` | — | USB-Serial-JTAG 原生 D+ |
| 24–30 | — | — | 内部高速 SPI Flash 总线，不可占用 |

USB Type-C 直连，同时承担控制台、二进制加载器和 JTAG 三个角色。

### 受保护引脚

内核用一张位掩码把上表锁死，U-Mode payload 通过 `gpio_*` 系统调用访问时一律返回 `-1`：

```
bit 1, 2        Clear CMOS 跳线
bit 3, 4        LP-Core Management Engine 地线 / sense
bit 8           WS2812
bit 9           BOOT 键
bit 12, 13      USB D- / D+
bit 24..30      SPI Flash 总线
```

留给用户 payload 的是 **GPIO 0、5、6、7、10、11、14–23**。

## Flash 分区

8MB Flash，自定义分区表（对应上游 `partitions.csv`）：

| 名称 | 类型 | 偏移 | 大小 | 用途 |
|---|---|---|---|---|
| `nvs` | data/nvs | 0x9000 | 64 KB | NVS |
| `otadata` | data/ota | 0x19000 | 8 KB | OTA 槽位标记 |
| `phy_init` | data/phy | 0x1b000 | 4 KB | RF 校准 |
| `bios_fw` | app/factory | 0x20000 | 1.5 MB | 主固件（A 槽） |
| `ota_0` | app/ota_0 | 0x1a0000 | 1.5 MB | OTA 被动槽（B 槽） |
| `openc6_fs` | data/0xff | 0x320000 | 4.875 MB | 日志式环形文件系统 |

## 运行时约定

- 控制台：USB-Serial-JTAG 原生 CDC，**不是** UART，也不是 RTT。
  这与本仓其它示例不同——`defmt` over RTT 只在开发期开，交互式 shell 走 USB。
- 默认 AP：SSID `BIOS_SETUP_C6`，网关 `192.168.4.1`，端口 80（BIOS Setup Web UI）。
- 默认 STA 配置存在 NVRAM 命名空间 `bios_cfg`，国家码缺省 `01`（World Safe Mode，信道 1–13）。
- 默认 PXE URL：`http://192.168.1.1:8080/payload.bin`。
- CPU 频率档位 80 / 120 / 160 MHz，通过 PCR `0x60096118` 直接改写分频。

## 移植状态

模块对照上游 `components/`，逐个模块的状态：

| 上游模块 | Rust 位置 | 状态 |
|---|---|---|
| `open_hal/targets/c6` | `src/hal/` | 已移植：GPIO / IO_MUX、PCR 调频、TIMG 看门狗、PMP、CSR、USB-Serial-JTAG、flash、复位溯源 |
| `nvram` | `src/nvram.rs` | 已移植，但改成固定布局结构体 + CRC32（上游用 IDF 的 NVS 键值库） |
| `openc6_fs` | `src/fs.rs` | 已移植，日志式环形、扇区 GC、墓碑删除 |
| `zc6` | `src/zc6/` | 已移植，v4 格式字节兼容；POST 时跑一次压缩→解压回环自检 |
| `sandbox` | `src/sandbox/` | 已移植：全局汇编 trap 向量表、`ecall` 分发、PMP arena、`setjmp`/`longjmp` 退出 |
| `boot_manager` | `src/shell.rs`、`src/proc.rs` | shell 与页池/进程表已移植 |
| `led_mgmt` | `src/led.rs` | 已移植，Aura 灯效走 embassy 任务 |
| `wifi_mgmt`、`web_ui`、`pxe_boot` | `src/net.rs`、`src/pxe.rs` | AP/STA、BIOS Setup Web UI、`/wsh` 远程 shell、PXE 下载已移植；STA + DHCP + PXE 已在板上实测通过 |
| `power_mgmt` | `src/power.rs`、`src/me.rs` | 已移植：AI Tweaker 应用 + 自治调频（`me::update_governor`，按忙闲比三档、升频立即/降频保持 1 s） |
| `management_engine`（LP-Core） | `src/me.rs`、`ulp/` | **部分**：LP 核已跑起来（心跳、电源键采集、喂狗停滞检出），检出后由 HP 侧执行复位；LP 侧直接触发复位未打通，见「已知缺口」 |

## 与上游的运行时差异

移植时踩出来的几处，都是**有意偏离上游**，不是漏做：

### 控制台是双路的

`hal::console` 同时往 **UART0**（ROM 已配好的 115200 8N1）和**原生 USB-Serial-JTAG** 写，
读也两路都收（优先 USB）。上游只写原生 USB，数据线插在 USB-UART 转接器上就完全看不到输出。

`hal::console::init()` 是**空实现**。上游那套 PCR 时钟/复位操作会重新枚举 USB，把 JTAG 侧
一起弄掉，探针再也连不上；写 `CHIP_RST` 屏蔽主机 DTR/RTS 更会让 `espflash` / `probe-rs`
没法把芯片复位回下载模式，只能拔线。ROM 配好的 USB-Serial-JTAG 原样用下去最省事。

### 掉电恢复默认是「自动开机」

上游 `DC_LOSS_POWER_OFF` 是默认值，因为它的安装器会预置 NVRAM。在开发板上这个默认值意味着
**插上电就自己关进 S5 深睡**，用户只看到一块死板。这里改成 `DC_LOSS_POWER_ON`，并且
`nvram::is_provisioned()` 为假（flash 里还没有通过校验的配置）时一律不执行掉电策略。

配套的判据也收紧了：`main` 里的自动 S5 现在要求 `is_provisioned()` **且**
`hal::power::is_power_on()`（即 `ResetReason::PowerOn`）。`espflash` 走 UART 复位芯片靠拉 EN 脚，
C6 会把这种复位报成 `ChipPowerOn`，光看复位原因区分不出「真上电」和「刚烧完」。

### 首次使用要先 `format`

`fs_init` 只在分区**全 0xFF** 时当作空白环。分区里如果残留着上一次固件/其它分区表写进去的
数据，会被当成有效环解析，读文件就是乱码。换固件或换分区表之后先敲一次 `format`。

### 已知缺口

这几处是有意的取舍，不是漏做：

1. **LP-Core 的复位动作**。esp-hal 1.2 **有**公开驱动 `esp_hal::lp_core::LpCore`
   （`ulp_riscv_driver_supported`），镜像、共享变量、心跳、电源键采集都已跑通，
   `wdtest` 也验证了「LP 核检出主核停喂 → HP 侧复位」整条链路。

   没打通的是**由 LP 核自己触发复位**。实测把 `lp_trigger_reset` 做成函数放在镜像后段，
   LP 核会停在原地（心跳冻结、芯片不复位）；改成宏内联展开后整个程序连入口都不执行
   （心跳恒 0）。已排除：镜像大小（356 B 纯填充版正常）、LP_WDT 寄存器访问（384 B 探针
   读写均正常）、LP RAM 可写性（含栈区）、栈帧本身。

   能复现的触发条件：给 `lp_main` 的循环顶部加一句
   `if read_shared(IS_ENABLED) == 0 { delay_tick(); continue; }`，心跳立刻变 0；
   去掉就恢复。也就是 LP 核对**循环里的提前 `continue` 分支**同样过敏。
   改 ULP 程序时保持最简线性结构，改完务必看 `info` 的 `LP heartbeat` 是否递增。

   所以现在 LP 核只做**采集与记录**，复位由 HP 侧 `me::poll_lp_requests()` 执行。
   后果：主核只是逻辑卡死（还能调度）时能自救；**主核彻底停摆时无法自救**。

2. **自治调频也在 HP 侧**。上游在 LP 核里直接写 PCR（`0x60096118`）改主频，这里做不到：
   LP 核能正常读写 **LP 外设空间**（`LP_IO 0x600B2000`、`LP_WDT 0x600B1C00` 都实测通过），
   但 PCR 属于 **HP 外设空间**，LP 核写入后 `clock::get_cpu_mhz()` 读回仍是原值。
   因此 `me::update_governor()` 每 100 ms 按忙闲比三档决策，升频立即生效、降频保持 1 s，
   并限制在 NVRAM 配置的频率上限内；`Performance` 策略直接锁上限。
3. **payload 的后台进程**。`boot <path> bg` 返回错误。U-Mode 会话要真正并发，得把沙箱放进
   独立的抢占式线程，并让 trap 路径能跨任务保存 `mscratch`。
4. **payload 的 BSD socket 系统调用**（`SYS_TCP_*`）。上游靠 lwIP 的阻塞 socket；embassy-net
   是异步的，在 trap 上下文里驱动不了执行器，所以返回 `-1`。内核侧的 Web UI / PXE / OTA 正常。
5. **ZSWAP 内存压缩**。`suspend` 只冻结进程状态，没有把 arena 页压缩回收。

另外：**烧录用 `espflash`，别用 `probe-rs`**。`probe-rs` 解析分区表时会在 `openc6_fs` 的
`0xff` 子类型上 panic，那是 `esp-idf-part 0.6.0` 的枚举缺项，不是分区表本身有错。
`probe-rs` 只用来 `attach` 看 RTT。

## Payload ABI

Payload 是位置无关的 RV32 扁平二进制，在 U-Mode 下运行，只能通过 `ecall` 访问内核服务。
ABI 结构、系统调用表、PMP 内存边界、编译参数见 [docs/payload_development.md](docs/payload_development.md)。

## 构建

```bash
cd openc6-bios-rs
cargo check           # 交叉检查，不碰板子
cargo run             # 构建 + 烧录 + RTT 日志（需探针）
```

`cargo run` 会通过 `probe-rs run --chip=esp32c6` 烧录。烧完之后交互式 shell 在 USB CDC 上，
用 `screen /dev/ttyACM1 115200` 或任何串口工具连接。

## 验证

PXE / OTA / 串口控制台需要一个「对面的服务端」，脚本都在 [test/](test/)，只依赖 Python 标准库：

```bash
python3 test/serve.py --dir /tmp --port 8001   # PXE 下载源，会打印可用 IPv4
python3 test/tcp_probe.py --port 8002          # 裸 TCP 探针，只判 SYN 到没到
python3 test/serial.py info                    # 串口跑一条 Shell 命令
```

```text
openc6_fs [/] /> pxe http://192.168.175.161:8001/payload.bin
downloading http://192.168.175.161:8001/payload.bin ...
52 bytes stored in /downloaded/payload.bin
```

排障顺序固定为「`tcp_probe.py` 有没有 `CONNECTED`」→「`serve.py` 有没有请求日志」：
前者决定是不是包没到本机（防火墙 / 射频），后者才轮到 HTTP 层。
