# max-98357a-amp

用 MAX98357A I2S 单声道功放模块轮流播放内嵌的两段测试音频，循环不停：

| 文件 | 格式 | 时长 | 大小 |
|---|---|---|---|
| `assets/videos/test.wav` | 44.1 kHz / 单声道 / 16 bit PCM | 8.7 s | 750 KB |
| `assets/videos/test.mp3` | 44.1 kHz / 单声道 / 64 kbps CBR（带封面） | 4 min 11 s | 2.0 MB |

先播 WAV，再播 MP3，一轮约 4 分 20 秒。

## 接线

| MAX98357A | nanoESP32-C6 | 说明 |
|---|---|---|
| VIN | 3V3 | 模块自带稳压，2.5–5.5 V 均可 |
| GND | GND | 必须共地 |
| BCLK | GPIO4 | I2S 位时钟（1.4112 MHz） |
| LRC | GPIO5 | I2S 声道时钟（44.1 kHz） |
| DIN | GPIO6 | I2S 数据 |
| SD | 悬空 | 悬空 = 使能，且按 (L+R)/2 混音；拉低则关断 |
| GAIN | 悬空 | 悬空 = 9 dB；接 GND = 15 dB，接 VIN = 3 dB |

喇叭接模块的 `+` / `-`（BTL 输出，**不能**把任一端接地）。

## 实现要点

- **构建需要交叉 C 工具链**：MP3 用 `threepm`（ThreePM，libhelix 派生的**定点**解码器）解，
  它是 C 库。C6 是 RV32IMAC，没有 FPU，纯 Rust 的浮点解码器（`nanomp3`、`sori-mp3-dec`、
  `mbop3`）在 160 MHz 上跑不动实时，只有定点实现能用。**先 `nix-shell` 再 `cargo`**。
- `.cargo/config.toml` 里 `AR_riscv32imac_unknown_none_elf` 必须是 binutils 的
  `riscv32-none-elf-ar`，用 `riscv32-none-elf-gcc-ar` 会失败；`CPATH` 指向 `src/cshim/include`
  的极简 C 头，用来挡住 nix 交叉 GCC 混进来的宿主 glibc 头（否则报 `gnu/stubs-32.h` 缺失）。
- I2S 走 `esp_hal::i2s::master`（Philips 标准、`Data16Channel16`）。采样率写死在构造时的
  时钟分频里，两段必须一致（有 `assert`）；声道数每段开播前用 `apply_config` 重配
  （单声道用 `Channels::MONO`，立体声用 `Channels::STEREO`）。
- WAV 头在运行时解析（`parse_wav`），MP3 用 `EasyMode` 解帧；两者都是立体声帧原样送出，
  由 MAX98357A 在片内混成单声道。
- 数据都在 flash（`include_bytes!`），DMA 读不了 flash，所以按 64 KB 一块拷进 `.bss` 的
  `DmaTxBuf` 再送。WAV 直接 `fill`；MP3 用**双缓冲**：一块在 DMA 播的同时把下一块解进另一个
  缓冲，否则解码那几十毫秒会变成每块一次的静音空档（实测听起来就是「卡」）。
- MP3 的 ID3v2 标签要自己先跳过（`skip_id3`）：`test.mp3` 的标签有 69 686 字节，远大于
  ThreePM 内部 2048 字节的环缓冲，不跳会让 `mp3_decode_ready` 反复返回 false，看起来像
  「没有数据」。
- 换音频：把新文件放到 `assets/videos/`，改 `src/main.rs` 里的 `WAV` / `MP3` 常量路径；
  两段必须同为 16 bit PCM WAV 与 CBR MP3，且采样率一致。

## 运行

```bash
cd max-98357a-amp
nix-shell shell.nix          # 提供 riscv32 交叉 GCC / binutils
cargo run --release          # 需要探针；日志走 RTT
```

启动时 RTT 会打印两段音频的格式与 MP3 的 ID3 大小，之后每段开始打一行
`round N: wav` / `round N: mp3`。

## 生成mp3音频：
- `ffmpeg -i input.mp3 -c:a libmp3lame -b:a 128k -ac 1 assets/videos/test.mp3`
