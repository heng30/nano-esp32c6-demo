# bilibili-follower

连上 WiFi，每分钟走 HTTPS 查一次 B 站某个 UP 主的粉丝数，把数字打进 defmt 日志。

- 接口：`https://api.bilibili.com/x/relation/stat?vmid=52374219`，取 JSON 里的 `data.follower`
- WiFi：`R3` / `visec*admin`（WPA2），DHCP 拿地址
- 周期：60 秒
- 不需要任何外设接线，板载 RGB 不参与

## 关键实现

| 部分 | 用的东西 | 说明 |
|---|---|---|
| TLS | `embedded-tls` 0.19（`Aes128GcmSha256`） | B 站只认 TLS 1.3 + RSA 证书，所以打开 `rsa` 特性；证书链不校验（见 `fetch_follower` 里的 `shortcut` 注释） |
| HTTP | 手写 `GET` 请求字符串 | 只取一个字段，没必要引 `reqwless`（它依赖的 `embedded-tls 0.18` 因 `der` 版本解析问题编不过） |
| JSON | 扫 `"follower":` 前缀再读数字 | 响应体（含 chunked 分帧）里该键和值始终连续，不引入 serde |
| 随机数 | `esp_hal::rng::Trng`（`TrngSource` 靠 SAR ADC 攒熵） | TLS 要密码学安全的随机源；ADC1 被它占着 |
| 堆 | `esp_alloc` 96 KiB | RSA 验签要在大整数上分配 |

`Cargo.toml` 里 `der` 的显式钉版是必须的，原因写在仓根 `Cargo.toml` 的注释里。

## 改什么

```rust
const SSID: &str = "R3";
const PASSWORD: &str = "visec*admin";
const REQUEST: &str = concat!(
    "GET /x/relation/stat?vmid=52374219 HTTP/1.1\r\n",  // 改 vmid 就能换人
    ...
);
const INTERVAL: Duration = Duration::from_secs(60);
```

## 跑

```bash
cd bilibili-follower
cargo run            # 构建 + 烧录 + RTT 日志（需探针）
```

实测日志：

```
[INFO ] joined `R3`
[INFO ] dhcp up: StaticConfigV4 { address: Cidr { address: 192.168.175.147, prefix_len: 24 }, gateway: Some(192.168.175.23), dns_servers: [192.168.175.23] } }
[INFO ] follower: 5055
```

## 换 AP 的坑

同一个口令在 `ow-blue` 上会一直 `AuthenticationExpired`（RSSI -64～-66，信号不算差），换到 `R3` 一次就连上。
ESP-IDF 这个错误码既可能是 PSK 对不上，也可能是 4 次握手超时；`esp-radio` 1.0.0-beta.1 只支持 WPA/WPA2，
AP 若是 WPA3/SAE 也会表现成这样。调这个时可以先 `DEFMT_LOG=debug cargo run` 看关联到哪一步。
