# test/ — 板级验证脚本

板子上的 PXE / OTA / Web UI 需要一个「对面的服务端」才能验证，这些脚本就是对面。
只依赖 Python 3 标准库，不需要额外装东西。

先把固件烧进去（见上层 `README.md`），然后：

## HTTP 服务端（PXE / OTA 下载源）

```bash
python3 test/serve.py --dir /tmp --port 8000
# 它会打印本机可用的 IPv4，板子侧照着拼 URL
```

```text
openc6_fs [/] /> pxe http://192.168.175.161:8000/payload.bin
downloading http://192.168.175.161:8000/payload.bin ...
52 bytes stored in /downloaded/payload.bin
```

`serve.py` 会把每次请求的来源 IP 打到 stdout。**服务端没打印 = 板子的包没到本机**，
不是 HTTP 层的问题，往下看 `tcp_probe.py`。

## 裸 TCP 探针（判定 SYN 有没有发出来）

```bash
python3 test/tcp_probe.py --port 8001
```

连接一建立就打印 `CONNECTED from <ip>:<port>`，不等应用层数据。用于区分：

| 现象 | 结论 |
|---|---|
| 有 `CONNECTED`，`serve.py` 收不到 | HTTP 层问题（URL、路径、响应解析） |
| `tcp_probe.py` 也收不到 | 包没到本机：防火墙 / 射频 / AP 客户端隔离 |

排查网络问题时按这个顺序：先 `tcp_probe.py` 确认包到得了，再换 `serve.py` 走完整 HTTP。

## 串口控制台

```bash
python3 test/serial.py info            # 跑一条命令
python3 test/serial.py wifi status
python3 test/serial.py --raw p.bin     # 把文件推给 Shell 的 serial 命令
python3 test/serial.py                 # 交互模式
```

默认走 `/dev/ttyACM1`（原生 USB-Serial-JTAG）。用 `--port` 改成 UART 转接器。

脚本内部先 `wait_prompt()` 再发命令：**复位后 `net::init` 会占住控制台一阵，
提示符出现前发的命令会被丢弃**，手工测试时踩过好几次。

## 已知的坑

- **`probe-rs attach` 会独占 USB-Serial-JTAG**，之后 CDC 控制台不再输出，只能复位恢复。
  抓 RTT 和用 `serial.py` 操作控制台不要并行。
- **NixOS 默认可能拦截入站端口**。板子 `pxe` 报「下载失败」而服务端毫无日志时，
  先在本机用 `curl` 自测服务端，再用另一台机器试；`tcp_probe.py` 无输出基本就是这个。
- **板子 `wifi connect` 之后要等 DHCP**，`wifi status` 的 `ip` 从 `-` → `0.0.0.0` →
  实际地址，弱信号下要几十秒。
