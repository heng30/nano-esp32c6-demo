# Minino → minio-rs 移植对照清单

真源：上游 README 功能清单。目标：**可行子集完整实现**（用户确认），
受阻功能显式 stub，不遗漏任何菜单项。

状态图例：✅ 完整实现 / 🟡 部分实现（原因注明）/ ⬛ stub 菜单项（技术上被挡死，显示"不可用"+原因）/
⬜ 待做

## Applications > WiFi

| Minino 功能 | 状态 | 说明 |
|---|---|---|
| Wardriving | 🟡 | 扫描完成（openc6 同款驻留 80–120ms 配置，结果打 RTT）；GPS 地理标注/SD 落盘等接线，扫描上板实测待用户按键 |
| WiFi Sniffer / Analyzer | ⬛ | esp-radio 无 promiscuous mode（闭源 blob），stub |
| WiFi Deauther | ⬛ | 需原始 802.11 帧注入，esp-radio 无 API，stub |
| CatDoS | ⬛ | 同上，stub |
| SSID Spam | ⬛ | AP 广播假 SSID 需要多 netif/原始注入，esp-radio 单 SSID，stub |
| Captive Portal | 🟡 | AP+DHCP+DNS劫持+HTTP 完成，表单录入 STA 凭据存 NV；**凭据已真正用于 STA 关联**（AP+STA 共存）：真机实测 ``R3`` → DHCP 192.168.175.147；手机测表单待用户 |
| DroneID | ⬛ | 依赖 promiscuous 抓帧，stub |

## Applications > Bluetooth

| Minino 功能 | 状态 | 说明 |
|---|---|---|
| BLE Spammer | 🟡 | 广播名轮播（Advertiser Drop 停播换名）已实现，待按键实测；上游的 SwiftPair/弹窗专用载荷格式未移植 |
| BLE HID Spoofing | 🟡 | 0x1812 服务手工搭建完成（Report Map/Report+Notify/0x2908 描述符），短按键入 minio-rs；配对实测待用户，部分手机可能要求加密配对（加密未接） |
| BLE ADV Sniffer | 🟡 | 常驻被动扫描 + 原始 adv 十六进制日志已实现，待按键实测；SD pcap 落盘等接线 |
| BLE Trackers Scanner | 🟡 | 分类器 8 项宿主单测全过（签名经公开逆向资料核实：FindMy 0x004C+0x12 长型分离态、Tile 0xFEED、SmartTag 0xFD5A 状态位、Chipolo 0xFE33/0x0157、FMDN 0xFEAA）；真机扫描待用户按键 |

## Applications > Spectrum / SDR

| Minino 功能 | 状态 | 说明 |
|---|---|---|
| RF Spectrum Analyzer（BARS/SCOPE/WATER/FOX/PEAK 5 视图） | ⬛ | 全部依赖 promiscuous 802.11 RSSI/帧统计，stub |
| Smart Anti-Jammer / RF IDS | ⬛ | 同上，stub |
| SDR 80 MS/s I/Q（RX/TX:PKT/TX:CW/HOST 四模式） | ⬛ | 需直捅 PHY 寄存器 + 链接期锁 128 KiB SRAM，且与 esp-radio blob 抢 radio；工作量与风险不成比例，stub |

## Applications > Zigbee / Thread

| Minino 功能 | 状态 | 说明 |
|---|---|---|
| IEEE 802.15.4 Sniffer over CLI | 🟡 | ieee154-build 形态实现：帧头解析 6 单测 + promiscuous 轮询 + 信道切换；esp-radio 编译期禁止与 WiFi 共存，双固件形态（见 README）；SD 落盘待接线，上板实测待用户 |
| Zigbee Spoofing | 🟡 | 同形态实现：Beacon Request 广播发送 |
| zigbee_light / zigbee_switch（加入 Zigbee 网络） | ⬛ | Rust 无 Zigbee 协议栈（无库），重写整套栈不现实，stub |
| Thread Broadcast / 6LoWPAN sniffer | 🟡 | 同形态实现：广播数据帧发送 + 共享嗅探 RX 路径 |

## Applications > 其它

| Minino 功能 | 状态 | 说明 |
|---|---|---|
| Surveillance Detect（BLE 指纹：Axon 记录仪、skimmer） | 🟡 | 设备名指纹匹配已实现（surveillance 模式）；WiFi 侧 OUI 指纹仍被 blob 挡死（⬛） |
| GPS | 🟡 | NMEA 解析器（GGA/RMC+校验和）已过 6 项单测；菜单已显式接入（报未接线原因）；UART 接入待 GPS 模块接线 |
| File Manager（Web 浏览器 + OLED 本地浏览） | 🟡 | Web `/files` 列 SD 根目录 + 短按 RTT 列目录已实现；无卡降级 503；OLED 本地浏览随屏接线 |

## Settings / About / Core

| Minino 功能 | 状态 | 说明 |
|---|---|---|
| Display（屏保风格/超时） | ✅ | 手写 SSD1306 驱动（ssd1306 crate 绑 eh 0.2 不可用）+ 屏保空闲清屏 + 按键唤醒；无屏降级路径已上板实测（no ack → 纯日志） |
| Menu Style（Classic/Icon List/Grid/Hero+Dock） | 🟡 | 四种风格渲染代码完成（5×8 字体、选中反白/Grid 双列/HeroDock 反白标题栏）；屏上实测等接线 |
| Logs Output | ✅ | defmt 级别由 `.cargo/config.toml` 的 `DEFMT_LOG` 控制（本仓既有约定） |
| SD Card Settings | 🟡 | 容量报告 + 自检写入（MINIO.TXT）实现，无卡降级已上板实测（sd card not found→纯日志）；真卡读写等接线；FAT32 格式化不做（挂载失败提示重格） |
| WiFi Settings | 🟡 | 凭据持久化 + AP+STA 共存自动关联 + **掉线自动重连**（盯 link 非 config，20 s 关联超时 + 5 s 退避；真机连 R3 验连续 4 轮 join→dhcp→断→重连）；录入 UI 仍走 portal 表单 |
| Stealth Mode | ✅ | 存储完成，RGB 灯联动（板上按键交互待用户确认） |
| Light Sleep | 🟡 | esp-hal LowPower::sleep_light+定时唤醒已接入；10 s 实测待用户按键 |
| Deep Sleep | ⬛ | 上游自己也是 TODO，对齐不做 |
| About（版本/许可/OTA） | 🟡 | 版本/致谢/OTA 状态三页已实现（`src/ota.rs`）；OTA 运行时读分区表判槽，esp-hal 默认单 factory 表下如实报缺 ota 槽 |
| app_manager 生命周期（on_enter/on_input/on_exit） | ✅ | MenuState+app 分发，树自检 38 条；OLED 渲染归任务 #3 |
| radio_lease 射频仲裁 | ✅ | 引用计数+互斥单测全过；promiscuous 租约恒 Unsupported（esp-radio 无此能力）；固件接入随 WiFi/BLE/802.15.4 app 落地 |
| 主机端测试 | ✅ | `minio-rs-core` 42 项宿主单测全过（menu 5 / lease 4 / nmea 6 / config 6 / pcap 3 / dns 4 / frame154 6 / trackers 8） |
