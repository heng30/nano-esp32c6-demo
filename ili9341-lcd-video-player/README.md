# ili9341-lcd-video-player

ESP32-C6 的无声视频播放器：AVI（MJPEG）文件内嵌在固件里，解出来满屏刷到 ILI9341 上，循环播放。

- 视频：`../assets/videos/test-320x240-without-audio.avi`（320×240、25 fps、218 帧、8.7 秒、无音轨）
- 解码：`zune-jpeg 0.5`（`no_std` + `alloc`，纯 Rust，无硬解），每帧一张 320×240 RGB 全解出来再转 RGB565 边转边刷
- 容器：本目录 `src/avi.rs` 自己解析 RIFF，只读 `avih`（宽高、每帧微秒）与 `movi`（`00dc` 帧块），不读 `idx1` 索引
- 屏：与 `ili9341-lcd` 同一套 16 位 8080 并口（`src/display.rs` 直接从那边复制），接线表见 [`../ili9341-lcd/README.md`](../ili9341-lcd/README.md)
- 日志：`defmt` over RTT

## 运行

```bash
cargo run --release
```

`release` 是必须的：debug 下解码慢到看不出是视频。

RTT 日志每轮循环打印一行播放结果：

```text
INFO  video 320x240, 218 frames, 40000 us/frame
INFO  played 218 frames in ... ms (... fps)
```

帧率没达到 25 是正常的：软解 320×240 一帧加上 18 根 GPIO 打满屏，比 40 ms 的帧间隔慢，
播放机会变慢而不是丢帧（`frame_delay` 只用做上限，解码超时不补帧）。
想接近原速只能降分辨率/降帧率重编码视频，换 `ffmpeg` 参数，不用改代码。

## 换视频

同一个 `ffmpeg` 命令行就能出这个格式：

```bash
ffmpeg -i input.mp4 -vf scale=320:240 -r 25 -c:v mjpeg -q:v 5 -pix_fmt yuvj420p -an output.avi
```

- 必须是 **320×240**（超过就只显示左上角，小于就留黑边）；`-an` 去掉音轨，本播放器不处理音频
- 换文件后改 `src/main.rs` 的 `VIDEO` 路径；体积直接进 flash（16 MB，够塞十几个）
- 只认 `LIST movi` 里按顺序排的 `<流号>dc` 帧块，OpenDML 分卷的多段 AVI 不支持

## 内存

- 一帧 RGB 缓冲 320×240×3 = 225 KB，从 `esp_alloc` 堆里来，堆设 288 KB
- 不做 RGB565 帧缓冲：解码结果直接 `chunks_exact(3)` 转 RGB565 流给屏，省下 150 KB，堆峰值就是那一张 RGB
- 视频本身在 `.rodata`（flash），不占 RAM；固件 flash 占用约 920 KB（其中视频 756 KB）

## 注意

- `zune-jpeg` 默认输出就是 RGB，不用配 `DecoderOptions`
- `JpegDecoder::new` 要的是 `ZCursor`，裸 `&[u8]` 不实现 `ZByteReaderTrait`，得用 `zune_jpeg::zune_core::bytestream::ZCursor` 包一层
- 解码失败直接 `expect` 崩掉：内嵌视频是构建产物，坏帧说明文件本身有问题，没必要带病继续跑
