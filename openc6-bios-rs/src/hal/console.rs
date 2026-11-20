//! USB-Serial-JTAG 裸金属控制台。
//!
//! 直接操作 EP1 FIFO 寄存器，绕开 RTOS 的 VFS 驱动——BIOS 要在中断屏蔽和
//! trap 上下文里都能打印。同时也是 `c6wsh` 远程终端的遥测抽头点。

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::regs::{read, write};

/// UART0 基地址（ROM 已按 115200 8N1 配好，直接写 FIFO 即可）。
const UART0_BASE: usize = 0x6000_0000;
/// UART0 发送 FIFO。
const UART0_FIFO: usize = UART0_BASE;
/// UART0 状态寄存器：`RXFIFO_CNT` 在 bit 0..9，`TXFIFO_CNT` 在 bit 16..25。
const UART0_STATUS: usize = UART0_BASE + 0x1C;
/// 发送 FIFO 的深度上限，留一点余量。
const UART0_TX_LIMIT: u32 = 120;

/// EP1 数据 FIFO。
const USB_EP1_DATA: usize = 0x6000_F000;
/// EP1 状态/命令寄存器。
const USB_EP1_CONF: usize = 0x6000_F004;

/// EP1_CONF：提交 FIFO 内容，让主机取包。
const USB_WR_DONE: u32 = 1 << 0;
/// EP1_CONF：IN 端点空闲，可以写下一包。
const USB_SERIAL_IN_FREE: u32 = 1 << 1;
/// EP1_CONF：OUT 端点有主机数据可读。
const USB_SERIAL_OUT_AVAIL: u32 = 1 << 2;

/// 单个 USB 包的最大字节数。
const PACKET: usize = 64;
/// 等 IN 端点空闲的轮询上限，超时即丢帧而不是卡死。
const POLL_LIMIT: u32 = 50_000;
/// 发包前先快速试一下的轮询上限。
const QUICK_POLL: u32 = 500;
/// 遥测抽头的自旋锁抢锁上限，超过就强行接管。
const SPIN_LIMIT: u32 = 1_000_000;

/// 控制台 FIFO 的自旋锁，避免多任务输出交错。
static LOCK: AtomicBool = AtomicBool::new(false);
/// 遥测抽头：把控制台字节复制给 `c6wsh` 的流式管道。
static TAP: AtomicUsize = AtomicUsize::new(0);

/// 初始化控制台。
///
/// 故意不写任何 USB 寄存器：ROM 已经把 USB-Serial-JTAG 配好，CDC 和 JTAG 两个接口
/// 都在工作。自己去动 PCR 的时钟/复位会重新枚举 USB，把 JTAG 侧一起弄掉，探针就再也
/// 连不上；写 `CHIP_RST` 屏蔽主机 DTR/RTS 更会让 `espflash` / `probe-rs` 没法把芯片
/// 复位回下载模式，只能拔线。这里只留一个空实现，把 ROM 的配置原样用下去。
pub fn init() {}

/// 挂上或摘掉遥测抽头。
pub fn set_tap(tap: Option<fn(&[u8])>) {
    TAP.store(tap.map_or(0, |f| f as usize), Ordering::Release);
}

/// 抽头当前是否挂着。
fn tap(bytes: &[u8]) {
    let addr = TAP.load(Ordering::Acquire);
    if addr != 0 {
        // SAFETY: 只可能由 `set_tap` 写入合法的 `fn(&[u8])` 指针。
        let f: fn(&[u8]) = unsafe { core::mem::transmute(addr) };
        f(bytes);
    }
}

fn lock() {
    let mut spin = 0u32;
    while LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        spin += 1;
        if spin > SPIN_LIMIT {
            LOCK.store(true, Ordering::Release);
            break;
        }
    }
}

fn unlock() {
    LOCK.store(false, Ordering::Release);
}

fn in_free() -> bool {
    read(USB_EP1_CONF) & USB_SERIAL_IN_FREE != 0
}

/// 等 IN 端点空闲，超时返回 `false`。
fn wait_in_free(limit: u32) -> bool {
    let mut left = limit;
    while !in_free() {
        left -= 1;
        if left == 0 {
            return false;
        }
    }
    true
}

/// 往 UART0 发送 FIFO 塞一个字节，满了就等。
fn uart_putc(c: u8) {
    while (read(UART0_STATUS) >> 16) & 0xFF >= UART0_TX_LIMIT {
        core::hint::spin_loop();
    }
    write(UART0_FIFO, c as u32);
}

/// 把一段字节镜像到 UART0。
fn uart_write(bytes: &[u8]) {
    for &b in bytes {
        uart_putc(b);
    }
}

fn putc_raw(c: u8) {
    write(USB_EP1_DATA, c as u32);
}

fn flush_raw() {
    write(USB_EP1_CONF, USB_WR_DONE);
}

/// 发一个字节。`\n` 自动补 `\r`。
pub fn putc(c: u8) {
    if !wait_in_free(QUICK_POLL) {
        return;
    }
    lock();
    if c == b'\n' {
        putc_raw(b'\r');
    }
    putc_raw(c);
    flush_raw();
    unlock();
    wait_in_free(POLL_LIMIT);
    uart_write(&[c]);
}

/// 打印一个字符串，`\n` 自动补 `\r`，每 64 字节切一包。
pub fn print(s: &str) {
    tap(s.as_bytes());
    uart_write(s.as_bytes());
    if !wait_in_free(QUICK_POLL) {
        return;
    }

    lock();
    let mut chunk = 0usize;
    for &b in s.as_bytes() {
        if chunk >= PACKET {
            flush_raw();
            chunk = 0;
            if !wait_in_free(POLL_LIMIT) {
                unlock();
                return;
            }
        }
        if b == b'\n' {
            putc_raw(b'\r');
            chunk += 1;
        }
        putc_raw(b);
        chunk += 1;
    }
    if chunk > 0 {
        flush_raw();
    }
    unlock();
    wait_in_free(POLL_LIMIT);
}

/// 发一段原始字节，不补 `\r`。
/// 是否有数据待读：原生 USB CDC 或 UART0 任一路有都算。
pub fn has_data() -> bool {
    read(USB_EP1_CONF) & USB_SERIAL_OUT_AVAIL != 0 || uart_rx_count() > 0
}

/// 读一个字节，优先原生 USB，其次 UART0。
pub fn getc() -> u8 {
    if read(USB_EP1_CONF) & USB_SERIAL_OUT_AVAIL != 0 {
        return (read(USB_EP1_DATA) & 0xFF) as u8;
    }
    (read(UART0_FIFO) & 0xFF) as u8
}

/// UART0 接收 FIFO 里待读的字节数。
fn uart_rx_count() -> u32 {
    read(UART0_STATUS) & 0x3FF
}

