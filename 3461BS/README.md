# 3461BS 四位共阳数码管

0.36 吋红色 4 位数码管（xlitx 3461BS），ESP32-C6 + esp-hal/embassy 动态扫描驱动，上电后 0–9999 每秒加一。

## 接线

段选串 220–330Ω 限流电阻；位选是共阳，直接由 GPIO 拉高（占空比 1/4，单段峰值电流很小）。

| 数码管脚 | 功能 | GPIO |  | 数码管脚 | 功能 | GPIO |
|---|---|---|---|---|---|---|
| 11 | A | 0 |  | 12 | DIG1 | 10 |
| 7  | B | 1 |  | 9  | DIG2 | 11 |
| 4  | C | 2 |  | 8  | DIG3 | 18 |
| 2  | D | 3 |  | 6  | DIG4 | 19 |
| 1  | E | 4 |  |    |      |    |
| 10 | F | 5 |  |    |      |    |
| 5  | G | 6 |  |    |      |    |
| 3  | DP| 7 |  |    |      |    |

引脚定义来自 `3461BS.pdf` 第 2 页内部接线图，与常见 3461AS/BS 丝印一致（面对正面、左下为 1 脚）。

## 运行

```sh
cargo run --release      # probe-rs 烧写并挂 RTT
# 或
espflash flash --port /dev/ttyACM0 target/riscv32imac-unknown-none-elf/release/display-3461bs
```

## 实现要点

- 段码表 `FONT` 的 bit0..7 = A B C D E F G DP，置位点亮；共阳所以段选拉低、位选拉高。
- 每次换位前先熄灭所有段（`sweep` 里的消隐），否则会有残影。
- 位选顺序即 `dig[i]`，第 i 位显示 `digits[i]`，`digits[0]` 是最左边（DIG1）。
