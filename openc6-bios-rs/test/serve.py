#!/usr/bin/env python3
"""PXE / OTA 验证用的 HTTP 文件服务端。

板子上的 `pxe <url>` 会来拉 payload，这里把请求和来源 IP 打到 stdout，
方便确认到底是板子没发、链路丢了，还是服务端没响应。

用法:
    python3 test/serve.py                     # 服务当前目录，监听 8000
    python3 test/serve.py --dir /tmp --port 8001

板子侧:
    pxe http://<本机IP>:8000/<文件名>
"""

import argparse
import http.server
import socket
import socketserver
import sys


class Handler(http.server.SimpleHTTPRequestHandler):
    """在标准静态文件服务基础上打印来源地址与字节数。"""

    def log_message(self, fmt, *args):
        print(f"[{self.log_date_time_string()}] {self.client_address[0]} {fmt % args}", flush=True)

    def end_headers(self):
        # 板子只需要拿完就断，禁用缓存避免二次请求拿到 304
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


class Server(socketserver.ThreadingTCPServer):
    """允许端口快速复用，反复重启服务端时不会卡在 TIME_WAIT。"""

    allow_reuse_address = True
    daemon_threads = True


def local_ips():
    """列出本机非回环 IPv4，用于拼出板子该访问的 URL。"""

    ips = set()
    try:
        for info in socket.getaddrinfo(socket.gethostname(), None, socket.AF_INET):
            ips.add(info[4][0])
    except OSError:
        pass
    # 走一次「连不上也拿不到包」的 UDP connect，取默认出口地址
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.connect(("192.0.2.1", 9))
            ips.add(s.getsockname()[0])
    except OSError:
        pass
    return sorted(i for i in ips if not i.startswith("127."))


def main():
    ap = argparse.ArgumentParser(description="PXE 验证用 HTTP 服务端")
    ap.add_argument("--port", type=int, default=8000)
    ap.add_argument("--dir", default=".")
    ap.add_argument("--bind", default="0.0.0.0")
    args = ap.parse_args()

    handler = lambda *a, **kw: Handler(*a, directory=args.dir, **kw)  # noqa: E731
    with Server((args.bind, args.port), handler) as httpd:
        print(f"serving {args.dir} on {args.bind}:{args.port}", flush=True)
        for ip in local_ips():
            print(f"  板子侧: pxe http://{ip}:{args.port}/<文件>", flush=True)
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nstopped", file=sys.stderr)


if __name__ == "__main__":
    main()
