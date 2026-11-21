# ble-led

用手机通过 BLE 控制板载 WS2812B 的开关：手机往灯输出特性写 ASCII `on` / `off`，
固件据此点亮或熄灭板上的 RGB 灯，读特性可以回读当前状态。

## 接线

| 灯 | 引脚 | 说明 |
|---|---|---|
| 板载 WS2812B | GPIO8 | DIN 直连，**无需外接任何元件**；用 `esp-hal-smartled` 经 RMT 驱动，颜色顺序 GRB |

## BLE 接口

| 项 | 值 |
|---|---|
| 广播名 | `ble-led` |
| 服务 UUID | `0x1815` Automation IO Service |
| 特性 UUID | `0x2BE2` Light Output |
| 特性属性 | Read / Write |
| 写入内容 | ASCII 文本 `on`（点亮，绿色）或 `off`（熄灭），其它内容忽略并回正为当前状态 |

## 使用

```bash
cargo run   # 构建 + 烧录 + RTT 日志（需探针）
```

1. 手机装 nRF Connect 之类的调试 App，扫描并连接 `ble-led`
2. 展开服务 `0x1815`，对特性 `0x2BE2` 写文本 `on` 或 `off`（Write，不是 Write Without Response）
3. RTT 日志会打印收到的指令；读该特性可以确认当前状态

## 自动测试

板子跑着固件时，主机侧可以跑 GATT 闭环测试（不碰探针，只用主机蓝牙）：

```bash
./test/gatt_test.sh                    # 按广播名 ble-led 找设备
./test/gatt_test.sh 42:4C:CA:5F:CA:15  # 或直接给 MAC
```

断言四项：写 `on` 读回 `on`、写 `off` 读回 `off`、非法值被拒、被拒后状态不变。
脚本靠主机 BlueZ 的 `busctl` 驱动，不依赖 `python-dbus`。两个坑写在脚本注释里：
busctl 直连 `Adapter1.StartDiscovery` 在本机多次 30 秒零命中，所以扫描这步交给后台的
`bluetoothctl --timeout 30 scan on`；扫到设备必须趁扫描还在跑立刻 `Connect`，
bluez 会回收只被扫到的临时设备对象，提前停扫描就 Connect 不到。

## 说明

- 开灯颜色是绿色、亮度 32/255，见 `src/main.rs` 的 `LED_ON_RGB`，别开满否则小板子上刺眼
- 没有配对与加密：连上就能写，只适合局域网/桌面示例
- 断连后自动重新广播
