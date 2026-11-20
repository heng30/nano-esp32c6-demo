#!/usr/bin/env python3
"""裸 TCP 监听器，用来判定板子的 SYN 到底有没有发出来。

比 HTTP 服务端更早给出信号：连接一建立就记一行，不等应用层数据。
排查「pxe 报下载失败」时先跑这个——如果没有 CONNECTED，说明包没到本机
（防火墙 / 射频 / 客户端隔离），而不是 HTTP 层的问题。

用法:
    python3 test/tcp_probe.py --port 8001
    # 板子侧: pxe http://<本机IP>:8001/anything
"""

import argparse
import socket
import time


def main():
    ap = argparse.ArgumentParser(description="裸 TCP 监听器")
    ap.add_argument("--port", type=int, default=8001)
    ap.add_argument("--bind", default="0.0.0.0")
    ap.add_argument("--reply", default="ok")
    args = ap.parse_args()

    with socket.socket() as srv:
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind((args.bind, args.port))
        srv.listen(5)
        print(f"listening {args.bind}:{args.port} at {time.strftime('%H:%M:%S')}", flush=True)

        while True:
            conn, addr = srv.accept()
            print(f"CONNECTED from {addr[0]}:{addr[1]} at {time.strftime('%H:%M:%S')}", flush=True)
            try:
                conn.settimeout(3)
                data = conn.recv(4096)
                print(f"  recv {len(data)}B: {data[:120]!r}", flush=True)
                body = args.reply.encode()
                conn.sendall(
                    b"HTTP/1.1 200 OK\r\n"
                    + f"Content-Length: {len(body)}\r\n".encode()
                    + b"Connection: close\r\n\r\n"
                    + body
                )
            except OSError as e:
                print(f"  err {e}", flush=True)
            finally:
                conn.close()


if __name__ == "__main__":
    main()
