# OpenC6 BIOS (Rust) 使用手册

本手册讲**怎么用**。移植细节、与上游的差异、已知缺口见 [README.md](./README.md)；
payload 的二进制格式与 ABI 见 [docs/payload_development.md](./docs/payload_development.md)。

---

## 1. 开始之前

### 1.1 烧录

```bash
cd openc6-bios-rs
cargo build --release

LD=/nix/store/dphsh3jff8w6n9p3xbfmkia5krqwv52f-glibc-2.42-103/lib/ld-linux-x86-64.so.2
ELF=../target/riscv32imac-unknown-none-elf/release/openc6-bios-rs

# 生成扁平镜像
$LD ~/.cargo/bin/espflash --skip-update-check save-image --chip esp32c6 "$ELF" app.bin

# 分区表 → 0x8000，应用 → 0x20000
$LD ~/.cargo/bin/espflash --skip-update-check write-bin --port /dev/ttyACM0 \
    --non-interactive 0x8000  partitions.bin
$LD ~/.cargo/bin/espflash --skip-update-check write-bin --port /dev/ttyACM0 \
    --non-interactive 0x20000 app.bin

$LD ~/.cargo/bin/espflash --skip-update-check reset --port /dev/ttyACM0
```

`/dev/ttyACM0` 是板载 UART 转接器（1a86:55d3），`/dev/ttyACM1` 是原生 USB-Serial-JTAG。
**控制台两路都有**，插哪个都能用；烧录建议走 `ttyACM0`（原生口在深睡时会消失，见 §11.3）。

> `probe-rs` 无法解析本项目的分区表（`esp-idf-part` 缺 `0xff` 子类型），烧录用 `espflash`。

### 1.2 连接控制台

```bash
# 串口终端，115200 8N1
minicom -D /dev/ttyACM0 -b 115200
```

启动后应看到：

```text
--- OpenC6 BIOS v2.0-ME (Rust) (ME Reason: None, RST Reason: PowerOn) ---
POST OK
OpenC6 Micro UNIX Shell. 输入 help 查看命令。
openc6_fs [/] /> 
```

`RST Reason` 说明本次启动的复位来源（`PowerOn` / `Software` / `DeepSleep` 等），
排障时先看这一行。

### 1.3 首次使用先 `format`

新烧的固件或换过分区表之后，`openc6_fs` 分区里可能残留旧数据，会被当成有效文件系统解析出乱码：

```text
openc6_fs [/] /> format
```

---

## 2. Shell 基础

| 操作 | 说明 |
|---|---|
| 提示符 | `openc6_fs [<当前目录>] /> `，根目录显示 `/` |
| 补全/历史 | 无（按行读取，回车执行） |
| 退格 | `0x08` 或 `0x7F` |
| `Ctrl+X` | 挂起当前前台进程 |
| `Ctrl+C` | 仅在上传（`serial`）尚未收到数据时中止 |

所有命令清单：

```text
openc6_fs [/] /> help
```

---

## 3. 文件系统

日志式环形 flash 文件系统，挂在 `openc6_fs` 分区（4.99 MB）。

### 3.1 浏览

```text
ls                      # 列当前目录
ls /downloaded          # 列指定目录
cd /downloaded          # 进目录
cd ..                   # 回上级
cat /a.txt              # 看内容
```

`ls` 输出：

```text
ID    TYPE NAME             SIZE
----------------------------------------
7     FILE zc6.bin          74
16    FILE spin.bin         4
2     DIR  downloaded       0
```

### 3.2 增删改

```text
mkdir /logs                    # 建目录
write /logs/note.txt hello     # 写文件（内容取第二个参数起的整行）
cat /logs/note.txt             # 验证
cp /a.txt /b.txt               # 复制
mv /b.txt /logs/c.txt          # 移动/改名
rm /logs/c.txt                 # 删除
```

路径同时支持绝对（`/downloaded/x.bin`）和相对当前目录（`x.bin`）两种写法。

### 3.3 格式化

```text
format
```

清空整个 `openc6_fs` 分区。**不可恢复**，只在文件系统损坏或首次使用时执行。

---

## 4. Payload 与沙箱

payload 是跑在 **U-Mode**（RISC-V 用户态）里的裸二进制，通过 PMP 与内核隔离。
越界访问内核内存会被硬件拦下，只杀进程、不崩系统。

### 4.1 运行

```text
boot /downloaded/payload.bin        # 前台运行，跑完回 Shell
boot /downloaded/payload.bin bg     # 只登记，不执行
```

成功的样子：

```text
launching /downloaded/payload.bin ...
OPENC6-SANDBOX-OK
sandbox session ended: Ok
pid 2 started
```

### 4.2 进程管理

```text
top                 # 进程表 + 页池
fg 2                # 把后台登记的 2 号进程真正跑起来
bg 2                # 转后台登记
kill 2              # 终止并回收
suspend 2           # 挂起（冻结状态）
```

作业控制是**协作式**的：`bg` 只登记，`fg` 才真正进 U-Mode 执行。
原因是单 hart 无抢占，改 trap 路径跨任务保存 `mscratch` 代价过大。

### 4.3 `.zc6` 透明解压

`boot` 支持直接跑 ZC6 压缩过的 payload，**不需要手动解压**——按文件头魔数自动识别：

```text
boot /downloaded/zc6.bin
```

### 4.4 沙箱被拦长什么样

payload 越权访问内核内存时：

```text
SIGSEGV: mcause=7 mtval=0x40800000
sandbox session ended: ErrFault
```

`mcause=7` 是 store access fault，`mtval` 是被拦下的地址。Shell 依然健康，可以继续用。

### 4.5 自己写 payload

格式、链接脚本、系统调用表见 [docs/payload_development.md](./docs/payload_development.md)。
可用的系统调用（`a7` 寄存器传号）：

| 号 | 名称 | 号 | 名称 |
|---|---|---|---|
| 0 | `exit` | 17 | `fs_read_file` |
| 1 | `print` | 18 | `fs_delete` |
| 2 | `delay_ms` | 19 | `sbrk` |
| 3 | `sys_reset` | 20–24 | `tcp_listen/accept/read/write/close` |
| 4 | `set_led_color` | 30 | `malloc` |
| 5 | `get_random` | 31 | `free` |
| 6 | `sha256` | 32 | `gpio_set_dir` |
| 7–9 | `math_isqrt/sin_deg/cos_deg` | 33 | `gpio_write` |
| 10 | `net_get_ip` | 34 | `gpio_read` |
| 12 | `wifi_is_connected` | | |
| 13–15 | `get_free_ram/total_ram/total_flash` | | |
| 16 | `fs_write_file` | | |

TCP 那 5 个当前返回 `-1`（embassy-net 是异步的，trap 上下文里驱动不了执行器）。

`gpio_*` 对受保护引脚一律返回 `-1`，可用的只有 **GPIO 0、5、6、7、10、11、14–23**。

---

## 5. 本地二进制上传

不走网络，直接把文件从电脑推给板子。

### 5.1 板子侧

```text
serial                       # 默认存到 /downloaded/payload.bin
serial /downloaded/my.bin    # 指定目标路径
```

出现 `receiving into ...` 后开始传。

### 5.2 电脑侧

```bash
python3 test/serial.py --raw /path/to/payload.bin
```

**协议要点**：没有任何结束符——板子靠**连续 750 ms 静默**判定传送结束。
所以 payload 里出现 `0x04`、`0xFF` 等任何字节都没问题；`0x03` 只在**尚未收到任何数据**时当中止。

---

## 6. 网络

### 6.1 作为 STA 连路由器

```text
wifi connect MySSID MyPassword    # 连接（凭据写进 NVRAM，重启自动连）
wifi status                       # 看模式 / IP / 状态
wifi scan                         # 扫周边 AP
wifi disconnect                   # 断开并退到 STA 空闲态
```

`wifi status` 输出：

```text
mode   : STA
ip     : 192.168.1.42
state  : connected
```

未连接时 `ip` 显示 `-`（不会拿上一轮的残留地址糊弄你）。

### 6.2 作为 AP 起 Setup 页面

```text
setup
```

```text
AP `BIOS_SETUP_C6` 已启动，密码 `12345678`
浏览 http://192.168.4.1/
```

| 项 | 值 |
|---|---|
| SSID | `BIOS_SETUP_C6` |
| 密码 | `12345678` |
| 网关 | `192.168.4.1` |

手机/电脑连上这个 AP 后即可访问 Web UI 和 Web Shell。
`setup` **不需要重启**，现场拉起；退出 AP 用 `wifi disconnect`。

---

## 7. BIOS Setup Web UI

浏览器打开 `http://192.168.4.1/`。

可调的项目包括：掉电恢复策略、POST 灯效、Aura 灯效与亮度、CPU 频率与调频策略、
欠压检测档位、ZC6 压缩模式、降频/紧急关机温度阈值、c6wsh 开关、Wi-Fi 国家码、
STA 的 SSID/密码、PXE URL。

页面按钮对应的接口：

| 接口 | 作用 |
|---|---|
| `GET /api/settings` | 读全部配置 |
| `POST /api/settings` | 写配置（落盘 NVRAM） |
| `GET /api/sysinfo` | 硬件/运行状态 |
| `POST /api/reboot` | 重启（先回应答再重启） |
| `POST /api/defaults` | 恢复出厂默认 |
| `POST /api/trigger_ota` | 标记下次启动执行 OTA |

---

## 8. Web Shell (`/c6wsh`)

浏览器打开 `http://192.168.4.1/c6wsh`，是一个黑底终端页面，可以直接远程敲 Shell 命令。

工作原理：

- 页面每 500 ms 向 `GET /c6wsh/stream?tail=<游标>` 拉一次增量
- 响应头 `X-Tail` 返回新游标，页面回填后续拉，**不会重复显示旧输出**
- 输入的命令 `POST /c6wsh/exec`，立即返回 `OK`，输出经 stream 流回来

手动验证（命令行）：

```bash
curl -i http://192.168.4.1/c6wsh/stream
# 响应头里有 X-Tail: N

curl -X POST -d 'help' http://192.168.4.1/c6wsh/exec
# 回 OK

curl -i "http://192.168.4.1/c6wsh/stream?tail=N"
# 只返回新增输出，新 X-Tail 比 N 大
```

> 已知限制：HTTP 是单连接串行处理，`exec` 同步执行命令。
> 跑长命令（如 `boot`、`pxe`）期间页面会卡住到命令结束，不是死锁。

还有一个简易接口 `GET /wsh?cmd=<命令>`，一次性返回最近 512 字节输出，适合脚本调用。

---

## 9. PXE 下载

从网络把 payload 拉进本地文件系统。

```text
pxe http://192.168.1.10:8000/payload.bin
```

```text
downloading http://192.168.1.10:8000/payload.bin ...
52 bytes stored in /downloaded/payload.bin
```

固定存到 `/downloaded/payload.bin`，可直接 `boot /downloaded/payload.bin`。

前置条件：STA 已连接（`wifi status` 看到 IP）。

准备服务端：

```bash
python3 test/serve.py --dir /tmp --port 8000
# 会打印本机可用 IPv4，照着拼 URL
```

主机名支持字面量 IP 和域名两种；写成 IP 时**不会**去走 DNS。

---

## 10. OTA 升级

两步：标记 → 重启执行。

1. Web UI 上点「触发 OTA」，或 `POST /api/trigger_ota`
2. 重启后固件从 NVRAM 里的 `pxe_url` 下载镜像，写进 `ota_0` 分区

```text
[OTA] 开始下载 BIOS 镜像...
[OTA] 镜像已写入被动槽
```

> shortcut: 当前直接按分区起点顺序写入，没做 ESP-IDF 镜像头校验和 A/B 槽轮转，
> 槽位标记与回滚交给 bootloader。

---

## 11. 电源、调频与温度

### 11.1 自治调频

由 HP 侧 `me::update_governor()` 每 100 ms 决策一次，按忙闲比切三档：

| 条件 | 频率 |
|---|---|
| 负载 ≥ 70% | 160 MHz |
| 负载 ≥ 20% | 120 MHz |
| 其余 | 80 MHz |
| 温度 ≥ 降频阈值 | 锁 80 MHz |
| `Performance` 策略 | 锁 NVRAM 上限 |

升频**立即**生效，降频有 **1 秒**保持（抖动抑制）。

```text
openc6_fs [/] /> info
CPU            : 120 MHz (LP 目标 120 MHz)
Governor       : Dynamic
CPU load       : 55 %
Temp           : 43 C
```

> 忙闲比是粗粒度采样（每条命令记一次忙、每 5 ms 空闲记一次闲），不是真实 CPU 占用率。
> 长时间跑 payload 期间 Shell 阻塞、两个计数都不涨，那个窗口会算成 0。

### 11.2 温度保护

由 `pet_watchdog()` 周期检查：

- 超过**降频阈值**（NVRAM，默认 55 °C）→ 锁到 80 MHz
- 超过**紧急阈值**（默认 75 °C）→ 直接进 S5

两个阈值都能在 Web UI 里改。

### 11.3 关机与深睡

```text
poweroff            # 真 S5，只能靠 RST / 拔插 USB 唤醒
poweroff 30         # 挂 RTC 闹钟，30 秒后自己醒来
reboot              # 热重启
```

**测试时一律用带秒数的写法**。原因：不带参数时只配了 ext1 唤醒（GPIO4 拉低），
而这块板子的 GPIO4 没接按键、只引到排针，睡下去就只能物理复位。

深睡期间的现象与控制台恢复手段：

| 现象 | 说明 |
|---|---|
| `/dev/ttyACM1` 消失 | 原生 USB-JTAG 断电，正常 |
| `/dev/ttyACM0` 仍在 | UART 转接器是独立芯片，不受影响 |
| 能否烧录 | **能**，对着 `ttyACM0` 烧录即可，DTR/RTS 走 EN 脚纯硬件复位 |
| 手动唤醒 | `RST` 键；`BOOT`+`RST` 进 ROM 下载模式；拔插 USB |
| 定时唤醒精度 | 请求 20 秒实测 21 秒（含 USB 重新枚举） |

唤醒后 `LP heartbeat` 从 0 重新计数，说明芯片确实复位重启了。

### 11.4 开机自动进 S5 的触发条件

要同时满足：真上电复位（`ResetReason::PowerOn`）+ NVRAM 已 provisioned +
`DcLossAction::PowerOff` + 无特殊启动原因。默认是 `PowerOn`，所以开发板上插电不会自己关机。

---

## 12. 管理引擎（LP 核）

ESP32-C6 的 LP 核跑一个 178 字节的独立镜像，与 HP 核通过 LP RAM 共享变量通信。

负责三件事：

1. **心跳**：`info` 里的 `LP heartbeat`，用来确认 LP 核真的在跑
2. **电源键去抖**：3 秒长按进 Setup，5 秒长按强制复位
3. **主核喂狗停滞检测**：HP 核 15 秒不喂狗就由 HP 侧执行复位

```text
openc6_fs [/] /> info
ME             : ACTIVE
LP heartbeat   : 5206
```

`LP heartbeat` 数值必须**持续增长**。恒为 0 说明 LP 核没跑起来（改过 `ulp/` 之后尤其要确认）。

### 12.1 验证看门狗链路

```text
wdtest 40
```

```text
已停止喂狗 40 秒；LP 看门狗阈值 15 秒，超时应复位芯片
[LP] 主核喂狗停滞超时，执行复位
```

接着芯片复位、重新启动。这条命令故意只停止喂狗而**不阻塞执行器**，
避免 esp-rtos 自己的看门狗抢先复位而掩盖 LP 侧的检测。

### 12.2 改 LP 程序的注意事项

- `lp_main` 必须紧跟在 `_start` 之后
- 循环里不要用提前 `continue` 的写法
- 改完必须用 `info` 确认 `LP heartbeat` 在涨

具体约束见 `ulp/src/main.rs` 顶部注释。

---

## 13. 诊断命令

```text
info     # CPU 频率/调频策略/负载/温度/ME/LP 心跳/heap/arena/ZC6/Wi-Fi
mem      # heap 总量、空闲、arena 用量
top      # 进程表与页池
```

`Arena 0 B` 是正常的——沙箱 arena 只在 `boot` 时才按需分配。

---

## 14. 测试脚本

`test/` 下三个脚本，只用 Python 标准库：

```bash
# PXE / OTA 的下载源，会打印本机可用 IPv4
python3 test/serve.py --dir /tmp --port 8000

# 裸 TCP 监听，只判断 SYN 有没有到达本机（查防火墙用）
python3 test/tcp_probe.py --port 8000

# 串口控制台驱动，自动探测 /dev/ttyACM*
python3 test/serial.py "info"                  # 发一条命令
python3 test/serial.py --send "boot x.bin"     # 只发不等
python3 test/serial.py --raw payload.bin       # 推裸字节给 serial 命令
```

排障顺序：先用 `tcp_probe.py` 确认板子的 SYN 到没到本机（没到就是网络/防火墙），
再用 `serve.py` 看有没有收到 HTTP 请求（到了但没响应就是应用层）。

---

## 15. 故障排查

| 现象 | 原因 / 处理 |
|---|---|
| `cat` 出乱码 | 分区里是残留数据，先 `format` |
| `error: 文件系统未挂载` | 同上 |
| 二进制上传后文件大小不对 | 电脑侧传完要有 750 ms 静默，脚本退出太快会提前截断 |
| `pxe` 报下载失败但服务端无日志 | 板子根本没连上网络，先 `wifi status` 看 IP |
| 有 IP 但 `pxe` 仍失败 | 主机防火墙拦入站，用 `tcp_probe.py` 确认 SYN 是否到达 |
| `LP heartbeat` 恒 0 | LP 镜像被改坏，回退 `ulp/` 到已知可用版本 |
| 深睡后控制台没了 | 正常；用 `RST` 唤醒，或提前用 `poweroff <秒>` |
| `probe-rs` 连不上 | 它解析不了本项目的分区表，用 `espflash` |
| 烧录报分区表错误 | `partitions.bin` 没写，补写 `0x8000` |
| 板子反复重启 | 查串口打印的 `RST Reason` 与管理引擎原因；`wdtest` 可复现看门狗路径 |

---

## 附：受保护引脚

`gpio_*` 系统调用对下列引脚一律返回 `-1`：

```text
bit 1, 2        Clear CMOS 跳线
bit 3, 4        LP-Core 管理引擎的地线 / sense
bit 8           WS2812
bit 9           BOOT 键
bit 12, 13      USB D- / D+
bit 24..30      SPI Flash 总线
```

留给 payload 的是 **GPIO 0、5、6、7、10、11、14–23**。
