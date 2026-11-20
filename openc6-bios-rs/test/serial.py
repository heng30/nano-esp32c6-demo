#!/usr/bin/env python3
"""板子串口控制台驱动。

固件把 Shell 同时镜像到原生 USB-Serial-JTAG 和 UART0，本脚本走前者
（`/dev/ttyACM1`，303a:1001），后者是 UART 转接器（`/dev/ttyACM0`）只用来烧录。

两个必须注意的点:
  * 复位后 `net::init` 会占住控制台一阵，提示符出现前的命令全部丢弃，
    所以发命令前一律先 `wait_prompt()`。
  * `probe-rs attach` 会独占 USB-Serial-JTAG，之后 CDC 控制台不再输出，
    只能复位恢复。抓 RTT 和用本脚本操作控制台不要同时进行。

用法:
    python3 test/serial.py info                 # 跑一条命令并打印输出
    python3 test/serial.py wifi status          # 多个参数拼成一条命令
    python3 test/serial.py --send 'boot x.bin'  # 原样发送，不等输出
    python3 test/serial.py --raw file.bin       # 把文件当裸字节推给 `serial`
"""

import argparse
import os
import select
import sys
import termios
import time

# 控制台双路（原生 USB + UART0），两路都会变成 /dev/ttyACM*，
# 枚举顺序会随复位/插拔变化，所以默认自动探测而不是写死。
DEFAULT_PORT = "auto"
PROMPT = "openc6_fs"


def _acm_ports():
    """按编号列出 /dev/ttyACM*。"""

    import glob

    return sorted(glob.glob("/dev/ttyACM*"))


def _try_open(dev, retries=25):
    """打开串口，复位后设备会短暂消失，给重试窗口。"""

    for _ in range(retries):
        try:
            return os.open(dev, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
        except PermissionError:
            time.sleep(0.4)
        except FileNotFoundError:
            return None
    return None


class Board:
    """一块通过 USB CDC 暴露 Shell 的板子。"""

    def __init__(self, port=DEFAULT_PORT):
        candidates = [port] if port != "auto" else _acm_ports()
        self.fd = None
        self.port = None
        for dev in candidates:
            fd = _try_open(dev)
            if fd is None:
                continue
            if self._looks_like_shell(fd):
                self.fd, self.port = fd, dev
                break
            os.close(fd)
        if self.fd is None:
            raise SystemExit(f"没找到带 Shell 的控制台（候选: {candidates}）")

        attr = termios.tcgetattr(self.fd)
        attr[0] = 0
        attr[1] = 0
        attr[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
        attr[3] = 0
        attr[4] = attr[5] = termios.B115200
        termios.tcsetattr(self.fd, termios.TCSANOW, attr)

    @staticmethod
    def _looks_like_shell(fd):
        """敲个回车看有没有 Shell 提示符。"""

        buf = b""
        for _ in range(6):
            try:
                os.write(fd, b"\r")
            except BlockingIOError:
                pass
            end = time.time() + 0.5
            while time.time() < end:
                r, _, _ = select.select([fd], [], [], 0.1)
                if r:
                    try:
                        buf += os.read(fd, 4096)
                    except BlockingIOError:
                        pass
            if PROMPT.encode() in buf:
                return True
        return False

    def _write(self, data):
        """非阻塞 fd 写入，遇到 EAGAIN 就退让重试。

        端口是以 O_NONBLOCK 打开的，直接 os.write 在发送缓冲满时会抛
        BlockingIOError——发命令和推 payload 都可能撞上。
        """

        view = memoryview(data)
        end = time.time() + 5
        while view and time.time() < end:
            try:
                n = os.write(self.fd, view)
                view = view[n:]
            except BlockingIOError:
                time.sleep(0.02)

    def drain(self, sec):
        """把 sec 秒内到达的输出一次性读出来。"""

        buf = b""
        end = time.time() + sec
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.15)
            if r:
                try:
                    chunk = os.read(self.fd, 8192)
                except BlockingIOError:
                    continue
                if chunk:
                    buf += chunk
        return buf.decode("utf-8", "replace")

    def wait_prompt(self, timeout=30):
        """反复敲回车直到看到 Shell 提示符。"""

        buf = ""
        end = time.time() + timeout
        while time.time() < end:
            buf += self.drain(0.5)
            if PROMPT in buf:
                return True
            self._write(b"\r")
        return False

    def send(self, line):
        """发送一行命令。"""

        self._write((line + "\r").encode())

    def cmd(self, line, wait=0.9, sec=2.5):
        """发送命令并把回显与结果一起去掉提示符后返回。"""

        self.send(line)
        time.sleep(wait)
        out = self.drain(sec)
        return out.replace(f"{PROMPT} [/] /> ", "").strip()

    def send_raw(self, data):
        """推送裸字节（供 Shell 的 `serial` 命令接收 payload）。"""

        self._write(data)

    def close(self):
        os.close(self.fd)


def main():
    ap = argparse.ArgumentParser(description="板子串口控制台驱动")
    ap.add_argument("--port", default=DEFAULT_PORT)
    ap.add_argument("--raw", help="把该文件的裸字节推给 Shell 的 serial 命令")
    ap.add_argument("--send", help="原样发送一行，不等待输出")
    ap.add_argument("--timeout", type=float, default=30.0, help="等提示符的秒数")
    ap.add_argument("--sec", type=float, default=2.5, help="发完命令后收集输出的秒数")
    ap.add_argument("words", nargs="*", help="要执行的 Shell 命令")
    args = ap.parse_args()

    board = Board(args.port)
    try:
        if not board.wait_prompt(args.timeout):
            print("等不到 Shell 提示符", file=sys.stderr)
            return 1

        if args.raw:
            with open(args.raw, "rb") as f:
                data = f.read()
            board.cmd(f"serial {os.path.basename(args.raw)}", wait=0.9, sec=1.0)
            board.send_raw(data)
            # 固件靠 750 ms 静默判定传输结束
            time.sleep(1.5)
            print(board.drain(1.5).replace(f"{PROMPT} [/] /> ", "").strip())
            return 0

        if args.send:
            board.send(args.send)
            return 0

        if args.words:
            print(board.cmd(" ".join(args.words), sec=args.sec))
            return 0

        # 没有任何参数时进交互模式，Ctrl+D 退出
        print("交互模式，Ctrl+D 退出")
        while True:
            try:
                line = input("> ")
            except (EOFError, KeyboardInterrupt):
                break
            if line.strip():
                print(board.cmd(line))
        return 0
    finally:
        board.close()


if __name__ == "__main__":
    sys.exit(main())
