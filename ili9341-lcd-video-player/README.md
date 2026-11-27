# ili9341-lcd-video-player

ESP32-C6 的无声视频播放器：AVI（MJPEG）内嵌在固件里，解出来 2× 放大铺满 ILI9341 循环播放，实测 **16 fps**。

- 视频：`../assets/videos/test-160x120-without-audio.avi`（160×120、25 fps、218 帧、无音轨），由 320×240 原片缩放重编码
- 解码：`zune-jpeg 0.5`（`no_std` + `alloc`，纯 Rust，无硬解），160×120 一帧 23.0 ms
- 写屏：**GPIO 直写 `out` 寄存器**的 8080-16 位并口，整屏 37.9 ms；掩码表放 RAM（放 flash 每像素多两次 XIP 读）
- 容器：`src/avi.rs` 自己解析 RIFF，只读 `avih`（宽高、每帧微秒）与 `movi`（`00dc` 帧块）
- 屏：接线与 `ili9341-lcd` 完全相同（16 根数据线 + RS + WR），接线表见 [`../ili9341-lcd/README.md`](../ili9341-lcd/README.md)
- 日志：`defmt` over RTT；屏右上角叠 `NN fps`

## 运行

```bash
cargo run --release
```

```text
INFO  ili9341 ready 320x240
INFO  video 160x120, 218 frames, 40000 us/frame
INFO  played 218 frames in 13288 ms (16 fps): decode 23014 us, draw 37903 us per frame
```

25 fps 的原片在这块芯片上跑不满：解码 23.0 ms + 整屏 37.9 ms ≈ 61 ms/帧。播放时**不丢帧、不慢放**——
拍数是原片的 16/25，看起来是慢动作。要接近原速只能重编码更低分辨率/帧率的视频。

## 三条写屏路径实测对比（24 fps 的预算是 41.6 ms/帧）

| 路径 | 整屏写 | 每帧合计 | 结果 |
|---|---|---|---|
| GPIO 直写 `out`（当前） | 37.9 ms | 61 ms | **16 fps，画面完全正确** |
| PARL_IO + DMA | 12–14 ms | 35 ms | 24 fps，但每次传输开头多打 ~68 个 WR 脉冲（真实锁存），一帧必须分 3 段（u16 长度限制）→ 3 处 68 像素错位 + 黑缝，无法消除 |
| I2S 并行 | — | — | esp-hal 1.2 里是 `#[cfg(esp32)]`，C6 上没有这个驱动 |

PARL_IO 的结论留在这里备查（想复活这条路时不用重测）：

- 16 位打包正确、位序正确：`TxSixteenBits::new(P0..P15)` 与 D0–D15 一一对应，
  `idle_value = 0x5a5a` 能原样从 16 根数据线读回 `0x5a5a`
- 单次传输时钟数 = 字节数/2 + 68：PCNT 数 WR 上升沿，25,600 像素实测 25,667~25,668 个边沿
  （开机后第一次 26,226，尾巴特别长）
- 短传输（2–10 字节的命令）靠不住，屏会被灌成别的命令 → 命令只能走 GPIO 打时序，
  而引脚交出去就回不来，所以 PARL_IO 版只能"初始化 GPIO + 像素 PARL_IO"
- WR 时钟要 `SampleEdge::Invert`：PARL_IO 在时钟上升沿换数据，ILI9341 在 WR 上升沿锁存

## 换视频

```bash
# 160×120：解码成本随像素数走
ffmpeg -i input.mp4 -vf scale=160:120:flags=area -r 25 -c:v mjpeg -q:v 5 -pix_fmt yuvj420p -an output.avi
```

- 必须是 **160×120**（解码按这个尺寸算行，别的尺寸会错行）；`-an` 去掉音轨
- 换文件后改 `src/main.rs` 的 `VIDEO` 路径
- 只认 `LIST movi` 里按顺序排的 `<流号>dc` 帧块，OpenDML 分卷的多段 AVI 不支持

## 内存

- 堆 256 KB：一帧解码 RGB 160×120×3 = 57.6 KB + 整帧 RGB565 153.6 KB（放大后一次性推屏）
- 没有块差分：整屏 37.9 ms 里总线占绝大部分，差分检测自身要扫 76,800 像素，省不回本
- 视频在 `.rodata`（flash），不占 RAM

## 注意

- 命令与像素共用同一次窗口设置：`Display::new` 里发一次整屏窗口 + `RAMWR`，之后 `draw_frame` 只送像素
- `Bus::write` 用 `out` 寄存器整体覆盖（影子值缓存其余引脚），一个像素两次寄存器写
- 方向不对（画面镜像）就把 `MADCTL_LANDSCAPE`（0x28）换成 `0xE8`，别去改引脚
- `zune-jpeg` 默认输出就是 RGB；`JpegDecoder::new` 要的是 `ZCursor`，裸 `&[u8]` 不实现 `ZByteReaderTrait`
