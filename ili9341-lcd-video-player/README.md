# ili9341-lcd-video-player

ESP32-C6 的无声视频播放器：AVI（MJPEG）内嵌在固件里，320×240 原片原样铺满 ILI9341 循环播放。

- 视频：`../assets/videos/test-320x240-without-audio.avi`（320×240、25 fps、218 帧、无音轨）
- 解码：`zune-jpeg 0.5`（`no_std` + `alloc`，纯 Rust，无硬解），一帧 65.0 ms
- 写屏：直接复用 `ili9341-lcd/src/display.rs`（GPIO 直写 `out` 寄存器 + `ili9341 0.6` 库驱动），整屏 51.9 ms
- 容器：`src/avi.rs` 自己解析 RIFF，只读 `avih`（宽高、每帧微秒）与 `movi`（`00dc` 帧块）
- 屏：接线与 `ili9341-lcd` 完全相同（16 根数据线 + RS + WR），接线表见 [`../ili9341-lcd/README.md`](../ili9341-lcd/README.md)
- 日志：`defmt` over RTT；屏右上角叠 `NNN fps`

## 运行

```bash
cargo run --release
```

```text
INFO  ili9341 ready 320x240
INFO  video 320x240, 218 frames, 40000 us/frame
INFO  played 218 frames in 25520 ms (8 fps): decode 65018 us, draw 51935 us per frame
```

播放时**不丢帧、不慢放**：218 帧就是一帧一帧全解全画完，只是帧率低于原片的 25 fps，看起来是慢动作。
要提高帧率只有两条路：换更小的分辨率（解码成本随像素数走），或换掉 GPIO 打时序的并口（见下）。

## 为什么不用 PARL_IO + DMA（本目录实测过，别再重走）

PARL_IO + DMA 能把整屏压到 12–16 ms，但推出来的像素流不是像素级准确的，两种时钟下两种坏法：

| WR 时钟 | 现象 | 复现方式 |
|---|---|---|
| 10 MHz | 纯色色块渲染成**向左倾斜的平行四边形**，整体缓慢右漂、闪屏 | 铺 3 段纯色（红/绿/蓝），本该是矩形 |
| 5 MHz | 斜纹 + 横向条纹网格（插入 `idle_value = 0x5a5a`，屏上看是偏蓝的灰） | 同上 |

配 PCNT 数 WR 上升沿：25,600 像素出了 25,667 个脉冲——多出来的锁存不是集中在传输开头（那样只会得到三段各自错位的矩形），而是散在整条流里，所以画面是斜的。**位置在软件侧不可推导，无法用偏移补偿**，这也是它与 GPIO 版唯一的差别。另外：

- 命令不能交给 PARL_IO：DC 为低时那些多余脉冲会被屏当成命令字节执行，命令序列必坏；引脚交出去又回不来，所以命令只能 GPIO 打时序
- `TxSixteenBits` 要的是 `PeripheralOutput`（直接把手里的 `Output` 交过去），不是 `PeripheralInput`
- `MAX_DMA_SIZE = 65535` 字节，即单次最多 32,767 像素；一帧 76,800 像素至少要 3 段
- `TxConfig` 是 `procmacros::BuilderLite`：`with_frequency` / `with_idle_value` / `with_sample_edge`
- I2S 并行（IDF 驱动 8080 LCD 的正统做法）在 esp-hal 1.2 是 `#[cfg(esp32)]`，C6 上没有

## 换视频

```bash
ffmpeg -i input.mp4 -vf scale=320:240:flags=area -r 25 -c:v mjpeg -q:v 5 -pix_fmt yuvj420p -an output.avi
```

- 必须是 **320×240**（和屏同样尺寸，解码结果直接铺满，不做缩放）
- 换文件后改 `src/main.rs` 的 `VIDEO` 路径
- 只认 `LIST movi` 里按顺序排的 `<流号>dc` 帧块，OpenDML 分卷的多段 AVI 不支持

## 内存

- 堆 368 KB：一帧解码 RGB 320×240×3 = 230.4 KB，`pack_rgb565` 在**同一块缓冲里就地压包**
  （第 i 像素写 2i、源在 3i，写指针永不越过读指针），所以不需要第二份帧缓冲；
  256 KB 会在帧尾分配失败（zune-jpeg 自己的系数缓冲还要约 24 KB）
- 视频在 `.rodata`（flash），不占 RAM

## 注意

- 屏驱动就是 `ili9341-lcd/src/display.rs`：本目录 `src/display.rs` 是它的副本，改那边要同步过来
- 窗口由驱动每次 `draw_raw_slice` 现设，写指针随像素递增，不做局部刷新
- 方向不对（画面镜像）改 `display::new` 里的 `Orientation`，别去动引脚
- `zune-jpeg` 默认输出就是 RGB；`JpegDecoder::new` 要的是 `ZCursor`，裸 `&[u8]` 不实现 `ZByteReaderTrait`
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
