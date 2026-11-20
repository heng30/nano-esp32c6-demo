//! U-Mode 沙箱：PMP 隔离的内存 arena、用户态入口，以及 `ecall` 系统调用分发。

pub mod trap;

use alloc::{
    alloc::{alloc, dealloc},
    boxed::Box,
    vec::Vec,
};
use core::{
    alloc::Layout,
    cell::RefCell,
    sync::atomic::{AtomicU32, AtomicUsize, Ordering},
};

use critical_section::Mutex;

use crate::{
    fs,
    hal::{console, cpu, gpio, pmp, power, wdt},
};

use self::trap::{JmpBuf, SANDBOX_SAVED_MTVEC, SANDBOX_VECTOR_BASE, TrapFrame};

/// 内核保留的 DRAM 字节数：Wi-Fi 基带、LwIP 缓冲和 RTOS 栈都要从里面出。
pub const DEFAULT_KERNEL_RESERVE: usize = 72 * 1024;
/// 每个沙箱任务专用的 M 模式 trap 栈大小。
pub const TRAP_STACK_SIZE: usize = 4 * 1024;
/// 内存块对齐粒度。
const BLOCK_ALIGN: usize = 16;
/// 沙箱堆里保留给内核 ABI 转发的尾部空间。
const ARENA_TAIL_RESERVE: usize = 8 * 1024;
/// 堆块魔数，用来识别非法 `free`。
const BLOCK_MAGIC: u32 = 0x5342_4D4F;

/// 系统调用号：退出并返回状态码。
const SYS_EXIT: u32 = 0;
/// 系统调用号：打印字符串。
const SYS_PRINT: u32 = 1;
/// 系统调用号：毫秒延时。
const SYS_DELAY_MS: u32 = 2;
/// 系统调用号：结束 payload 会话。
const SYS_SYS_RESET: u32 = 3;
/// 系统调用号：设置 WS2812 颜色。
const SYS_SET_LED_COLOR: u32 = 4;
/// 系统调用号：取硬件随机数。
const SYS_GET_RANDOM: u32 = 5;
/// 系统调用号：SHA-256。
const SYS_SHA256: u32 = 6;
/// 系统调用号：整数平方根。
const SYS_MATH_ISQRT: u32 = 7;
/// 系统调用号：定点正弦。
const SYS_MATH_SIN_DEG: u32 = 8;
/// 系统调用号：定点余弦。
const SYS_MATH_COS_DEG: u32 = 9;
/// 系统调用号：取本机 IP。
const SYS_NET_GET_IP: u32 = 10;
/// 系统调用号：Wi-Fi 是否已连上。
const SYS_WIFI_IS_CONNECTED: u32 = 12;
/// 系统调用号：空闲 RAM 字节数。
const SYS_GET_FREE_RAM: u32 = 13;
/// 系统调用号：RAM 总量。
const SYS_GET_TOTAL_RAM: u32 = 14;
/// 系统调用号：flash 总量。
const SYS_GET_TOTAL_FLASH: u32 = 15;
/// 系统调用号：写文件。
const SYS_FS_WRITE_FILE: u32 = 16;
/// 系统调用号：读文件。
const SYS_FS_READ_FILE: u32 = 17;
/// 系统调用号：删文件。
const SYS_FS_DELETE: u32 = 18;
/// 系统调用号：调整堆断点。
const SYS_SBRK: u32 = 19;
/// 系统调用号：TCP 监听。
const SYS_TCP_LISTEN: u32 = 20;
/// 系统调用号：TCP accept。
const SYS_TCP_ACCEPT: u32 = 21;
/// 系统调用号：TCP 读。
const SYS_TCP_READ: u32 = 22;
/// 系统调用号：TCP 写。
const SYS_TCP_WRITE: u32 = 23;
/// 系统调用号：TCP 关闭。
const SYS_TCP_CLOSE: u32 = 24;
/// 系统调用号：堆分配。
const SYS_MALLOC: u32 = 30;
/// 系统调用号：堆释放。
const SYS_FREE: u32 = 31;
/// 系统调用号：设置 GPIO 方向。
const SYS_GPIO_SET_DIR: u32 = 32;
/// 系统调用号：GPIO 输出。
const SYS_GPIO_WRITE: u32 = 33;
/// 系统调用号：GPIO 输入。
const SYS_GPIO_READ: u32 = 34;

/// `mcause` 里 U-Mode `ecall` 的编码。
const MCAUSE_USER_ECALL: u32 = 8;

/// 沙箱会话结束状态，数值与上游 `sandbox_status_t` 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum Status {
    /// payload 正常结束。
    Ok,
    /// payload 主动请求内核去连 Wi-Fi。
    ReqWifiConnect,
    /// 镜像超过 arena 容量。
    ErrSize,
    /// PMP 访问违规或访存错误。
    ErrFault,
    /// 非法指令或未处理的 trap。
    ErrInvalidOp,
    /// arena 分配失败。
    ErrNoMem,
}

impl Status {
    fn from_i32(v: i32) -> Self {
        match v {
            -1 => Self::ErrSize,
            -2 => Self::ErrFault,
            -3 => Self::ErrInvalidOp,
            -4 => Self::ErrNoMem,
            1 => Self::ReqWifiConnect,
            _ => Self::Ok,
        }
    }

    fn as_i32(self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::ReqWifiConnect => 1,
            Self::ErrSize => -1,
            Self::ErrFault => -2,
            Self::ErrInvalidOp => -3,
            Self::ErrNoMem => -4,
        }
    }
}

/// 堆块头，紧跟其后的才是给 payload 的地址。
#[repr(C)]
struct BlockHeader {
    size: usize,
    next: *mut BlockHeader,
    magic: u32,
}

/// 沙箱全部可变状态。
struct Sandbox {
    arena: &'static mut [u8],
    heap_break: usize,
    free_list: *mut BlockHeader,
    exit_status: i32,
    trap_stack: Box<[u8; TRAP_STACK_SIZE]>,
}

// SAFETY: 沙箱状态只由当前正在跑 payload 的那个任务访问，跨任务访问都经过临界区。
unsafe impl Send for Sandbox {}

static SANDBOX: Mutex<RefCell<Option<Sandbox>>> = Mutex::new(RefCell::new(None));
static ACTIVE: AtomicU32 = AtomicU32::new(0);
static SAVED_MSCRATCH: AtomicU32 = AtomicU32::new(0);
static SAVED_MIE: AtomicU32 = AtomicU32::new(0);
static EXIT_JMP: AtomicUsize = AtomicUsize::new(0);

fn with<T>(f: impl FnOnce(&mut Sandbox) -> T) -> Option<T> {
    critical_section::with(|cs| {
        let mut guard = SANDBOX.borrow_ref_mut(cs);
        guard.as_mut().map(f)
    })
}

/// 分配一块 16 字节对齐的 arena。
fn alloc_arena(size: usize) -> Option<&'static mut [u8]> {
    let layout = Layout::from_size_align(size, BLOCK_ALIGN).ok()?;
    let ptr = unsafe { alloc(layout) };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: 刚按这个 layout 分配出来，独占所有权。
    Some(unsafe { core::slice::from_raw_parts_mut(ptr, size) })
}

/// 分配沙箱 arena 并配好 PMP 窗口。
pub fn init(kernel_reserve_bytes: usize) -> bool {
    if with(|_| ()).is_some() {
        return true;
    }

    let free = esp_alloc::HEAP.free();
    if free <= kernel_reserve_bytes {
        defmt::error!(
            "[SANDBOX] DRAM 不足: free={} B, reserve={} B",
            free,
            kernel_reserve_bytes
        );
        return false;
    }

    let size = (free - kernel_reserve_bytes) & !(BLOCK_ALIGN - 1);
    let Some(arena) = alloc_arena(size) else {
        defmt::error!("[SANDBOX] arena 分配失败 ({} B)", size);
        return false;
    };

    let start = arena.as_ptr() as usize;
    let end = start + arena.len();
    pmp::setup(start, end);

    defmt::info!(
        "[SANDBOX] arena 就绪: {} KB [{:#x} - {:#x}] (保留 {} KB)",
        size / 1024,
        start,
        end,
        kernel_reserve_bytes / 1024
    );

    let sb = Sandbox {
        heap_break: start,
        arena,
        free_list: core::ptr::null_mut(),
        exit_status: 0,
        trap_stack: Box::new([0u8; TRAP_STACK_SIZE]),
    };
    critical_section::with(|cs| *SANDBOX.borrow_ref_mut(cs) = Some(sb));
    true
}

/// 释放 arena 并把 PMP 复位。
pub fn deinit() {
    pmp::clear();
    let arena = with(|sb| {
        let a = sb.arena.as_mut_ptr();
        (a, sb.arena.len())
    });
    critical_section::with(|cs| *SANDBOX.borrow_ref_mut(cs) = None);
    if let Some((ptr, len)) = arena {
        if let Ok(layout) = Layout::from_size_align(len, BLOCK_ALIGN) {
            // SAFETY: 这块内存是 `alloc_arena` 用同一 layout 分配的，且已从状态里摘除。
            unsafe { dealloc(ptr, layout) };
        }
    }
    ACTIVE.store(0, Ordering::Release);
    defmt::info!("[SANDBOX] arena 已释放");
}

/// arena 起始地址与长度。
pub fn arena() -> Option<(*mut u8, usize)> {
    with(|sb| (sb.arena.as_mut_ptr(), sb.arena.len()))
}

/// arena 容量。
pub fn arena_size() -> usize {
    with(|sb| sb.arena.len()).unwrap_or(0)
}

/// 沙箱会话是否正在跑。
/// 把 payload 镜像拷进 arena 起始处，并重置堆断点。
pub fn load_payload(src: &[u8]) -> bool {
    with(|sb| {
        if src.len() > sb.arena.len() {
            return false;
        }
        sb.arena[..src.len()].copy_from_slice(src);
        sb.heap_break = sb.arena.as_ptr() as usize + ((src.len() + BLOCK_ALIGN - 1) & !(BLOCK_ALIGN - 1));
        sb.free_list = core::ptr::null_mut();
        sb.exit_status = 0;
        true
    })
    .unwrap_or(false)
}

/// 在 U-Mode 里跑当前 arena 里的 payload，阻塞到它结束。
///
/// 返回 `None` 表示 arena 还没初始化。
pub fn run() -> Option<Status> {
    let (entry, user_sp, trap_sp, arena_len) = with(|sb| {
        let base = sb.arena.as_ptr() as usize;
        let trap_sp = sb.trap_stack.as_mut_ptr() as usize + TRAP_STACK_SIZE;
        // 用户栈从 arena 顶端向下长，给内核 ABI 留出尾部空间
        let user_sp = base + sb.arena.len() - ARENA_TAIL_RESERVE;
        (base, user_sp, trap_sp, sb.arena.len())
    })?;

    if arena_len == 0 {
        return Some(Status::ErrNoMem);
    }

    // 接管 trap 向量；原表地址留给中断转发用
    let saved_mtvec = cpu::swap_mtvec_vectored(vector_table_addr());
    // 原始值（含模式位）留给恢复；掩掉低两位的基址留给中断转发
    SANDBOX_SAVED_MTVEC.store(saved_mtvec, Ordering::Release);
    SANDBOX_VECTOR_BASE.store(saved_mtvec & !0x3, Ordering::Release);
    SAVED_MSCRATCH.store(cpu::read_mscratch(), Ordering::Release);
    SAVED_MIE.store(cpu::read_mie(), Ordering::Release);
    wdt::timg_save_and_disable();
    ACTIVE.store(1, Ordering::Release);

    let mut jmp = JmpBuf::default();
    // SAFETY: 上下文只在本函数栈帧存活期间被 longjmp 回来。
    let resumed = unsafe { trap::sandbox_setjmp(&mut jmp) };
    if resumed == 0 {
        EXIT_JMP.store(&mut jmp as *mut JmpBuf as usize, Ordering::Release);
        // SAFETY: arena 已在上面校验过，entry 指向刚拷进去的镜像首字节。
        unsafe { cpu::enter_umode(entry, user_sp, 0, trap_sp) }
    }

    // 从 trap 或正常返回路径回到这里
    EXIT_JMP.store(0, Ordering::Release);
    restore_host();
    Some(Status::from_i32(with(|sb| sb.exit_status).unwrap_or(0)))
}

/// 把宿主执行环境从沙箱接管状态恢复回来。
fn restore_host() {
    cpu::write_mscratch(SAVED_MSCRATCH.load(Ordering::Acquire));
    cpu::write_mtvec(SANDBOX_SAVED_MTVEC.load(Ordering::Acquire));
    cpu::write_mie(SAVED_MIE.load(Ordering::Acquire));
    cpu::restore_gp();
    cpu::force_mmode_mpp();
    wdt::timg_restore();
    ACTIVE.store(0, Ordering::Release);
    // trap 进入时硬件清了 MIE，longjmp 出来没有 mret，必须显式开回来
    cpu::irq_enable();
}

/// 取向量表地址。
fn vector_table_addr() -> usize {
    unsafe extern "C" {
        static sandbox_vector_table: u32;
    }
    (&raw const sandbox_vector_table).addr()
}

/// 结束当前会话，把控制权交回 [`run`]。
pub fn request_exit(status: Status) -> ! {
    with(|sb| sb.exit_status = status.as_i32());
    let jmp = EXIT_JMP.load(Ordering::Acquire);
    if jmp == 0 {
        // 没有可用的恢复点，只能硬重启
        power::reboot();
    }
    // 宿主状态统一由 `run` 在 longjmp 回来后恢复，这里不重复做
    // SAFETY: `jmp` 由 `run` 在调用 `enter_umode` 前写入，指向仍然有效的栈帧。
    unsafe { trap::sandbox_longjmp(jmp as *mut JmpBuf) }
}

/// trap 分发入口，由 `sandbox_trap_entry` 汇编调用。
///
/// # Safety
/// 只能由 trap 汇编调用，`frame` 必须指向 144 字节的合法现场。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sandbox_dispatch_trap(frame: *mut TrapFrame, mcause: u32, mtval: u32) {
    let frame = unsafe { &mut *frame };

    if mcause == MCAUSE_USER_ECALL {
        let ret = dispatch_syscall(frame);
        frame.a0 = ret as u32;
        frame.mepc += 4;
        return;
    }

    // 其余全是致命 trap：报诊断信息后结束会话
    console::print("\r\n======================================================================\r\n");
    console::print("[SANDBOX] U-Mode 违规被 PMP 拦截\r\n");
    console::print("======================================================================\r\n");
    let mut line = alloc::format!(
        "  MEPC   : 0x{:08X}\r\n  MCAUSE : 0x{:08X} (code {})\r\n  MTVAL  : 0x{:08X}\r\n",
        frame.mepc,
        mcause,
        mcause & 0x7FFF_FFFF,
        mtval
    );
    console::print(&line);
    line.clear();

    // mcause 5 = Load access fault, 7 = Store access fault
    let status = if mcause == 5 || mcause == 7 {
        Status::ErrFault
    } else {
        Status::ErrInvalidOp
    };
    request_exit(status);
}

/// 处理一条 `ecall`，返回值写回 `a0`。
fn dispatch_syscall(frame: &TrapFrame) -> i32 {
    match frame.a7 {
        SYS_EXIT => {
            let code = frame.a0 as i32;
            request_exit(Status::from_i32(code));
        }
        SYS_SYS_RESET => request_exit(Status::Ok),
        SYS_PRINT => {
            if let Some(s) = user_str(frame.a0, 512) {
                console::print(s);
            }
            0
        }
        SYS_DELAY_MS => {
            busy_delay_ms(frame.a0);
            0
        }
        SYS_SET_LED_COLOR => {
            crate::led::set_color(frame.a0 as u8, frame.a1 as u8, frame.a2 as u8);
            0
        }
        SYS_GET_RANDOM => esp_hal::rng::Rng::new().random() as i32,
        SYS_SHA256 => {
            if let Some(input) = user_bytes(frame.a0, frame.a1 as usize) {
                let digest = sha256(input);
                if let Some(out) = user_bytes_mut(frame.a2, 32) {
                    out.copy_from_slice(&digest);
                    0
                } else {
                    -1
                }
            } else {
                -1
            }
        }
        SYS_MATH_ISQRT => isqrt(frame.a0) as i32,
        SYS_MATH_SIN_DEG => sin_deg(frame.a0 as i32),
        SYS_MATH_COS_DEG => cos_deg(frame.a0 as i32),
        SYS_NET_GET_IP => crate::net::copy_ip(user_bytes_mut(frame.a0, frame.a1 as usize)),
        SYS_WIFI_IS_CONNECTED => crate::net::is_connected() as i32,
        SYS_GET_FREE_RAM => esp_alloc::HEAP.free() as i32,
        SYS_GET_TOTAL_RAM => (esp_alloc::HEAP.used() + esp_alloc::HEAP.free()) as i32,
        SYS_GET_TOTAL_FLASH => crate::hal::flash::chip_size() as i32,
        SYS_FS_WRITE_FILE => {
            let name = user_str(frame.a0, 64).unwrap_or("");
            let data = user_bytes(frame.a1, frame.a2 as usize).unwrap_or(&[]);
            match fs::write_file(name, data, frame.a3 as u16) {
                Some(_) => 0,
                None => -1,
            }
        }
        SYS_FS_READ_FILE => {
            let name = user_str(frame.a0, 64).unwrap_or("");
            let Some(id) = fs::find_id(name, frame.a4 as u16) else {
                return -1;
            };
            let Some(dest) = user_bytes_mut(frame.a1, frame.a3 as usize) else {
                return -1;
            };
            fs::read_file(id, frame.a2, frame.a3, dest)
        }
        SYS_FS_DELETE => {
            let name = user_str(frame.a0, 64).unwrap_or("");
            if let Some(id) = fs::find_id(name, frame.a1 as u16) {
                fs::delete(id);
            }
            0
        }
        SYS_SBRK => sandbox_sbrk(frame.a0 as i32),
        SYS_MALLOC => sandbox_malloc(frame.a0 as usize) as i32,
        SYS_FREE => {
            sandbox_free(frame.a0 as *mut u8);
            0
        }
        SYS_GPIO_SET_DIR => gpio::user_set_dir(frame.a0, frame.a1 != 0),
        SYS_GPIO_WRITE => gpio::user_write(frame.a0, frame.a1 != 0),
        SYS_GPIO_READ => gpio::user_read(frame.a0),
        SYS_TCP_LISTEN | SYS_TCP_ACCEPT | SYS_TCP_READ | SYS_TCP_WRITE | SYS_TCP_CLOSE => {
            crate::net::tcp_syscall(frame.a7, &[frame.a0, frame.a1, frame.a2, frame.a3])
        }
        _ => -1,
    }
}

/// 取用户地址处的 C 字符串。只接受 arena 内的地址。
fn user_str(addr: u32, max: usize) -> Option<&'static str> {
    let bytes = user_bytes(addr, max)?;
    let end = bytes.iter().position(|&b| b == 0)?;
    core::str::from_utf8(&bytes[..end]).ok()
}

/// 取 arena 内一段只读字节。
fn user_bytes(addr: u32, len: usize) -> Option<&'static [u8]> {
    let (start, size) = arena()?;
    let base = start as usize;
    let addr = addr as usize;
    if addr < base || addr + len > base + size {
        return None;
    }
    // SAFETY: 地址已确认落在 arena 内，arena 生命周期贯穿整个会话。
    Some(unsafe { core::slice::from_raw_parts(addr as *const u8, len) })
}

/// 取 arena 内一段可写字节。
fn user_bytes_mut(addr: u32, len: usize) -> Option<&'static mut [u8]> {
    let (start, size) = arena()?;
    let base = start as usize;
    let addr = addr as usize;
    if addr < base || addr + len > base + size {
        return None;
    }
    // SAFETY: 同上；同一时刻只有一个 payload 在跑，不存在别名。
    Some(unsafe { core::slice::from_raw_parts_mut(addr as *mut u8, len) })
}

/// 用 systimer 忙等，避免在 trap 上下文里进调度器。
fn busy_delay_ms(ms: u32) {
    let deadline = esp_hal::time::Instant::now() + esp_hal::time::Duration::from_micros(ms as u64 * 1000);
    while esp_hal::time::Instant::now() < deadline {
        core::hint::spin_loop();
    }
}

/// arena 内的堆分配。
fn sandbox_malloc(size: usize) -> *mut u8 {
    with(|sb| {
        if size == 0 {
            return core::ptr::null_mut();
        }
        let aligned = (size + BLOCK_ALIGN - 1) & !(BLOCK_ALIGN - 1);
        let total = aligned + core::mem::size_of::<BlockHeader>();

        // 先试空闲链表
        let mut prev: *mut BlockHeader = core::ptr::null_mut();
        let mut cur = sb.free_list;
        while !cur.is_null() {
            // SAFETY: 链表节点都是自己分配的，指针有效。
            let node = unsafe { &mut *cur };
            if node.size >= aligned {
                if prev.is_null() {
                    sb.free_list = node.next;
                } else {
                    unsafe { (*prev).next = node.next };
                }
                node.magic = BLOCK_MAGIC;
                return unsafe { (cur as *mut u8).add(core::mem::size_of::<BlockHeader>()) };
            }
            prev = cur;
            cur = node.next;
        }

        let limit = sb.arena.as_ptr() as usize + sb.arena.len() - ARENA_TAIL_RESERVE;
        if sb.heap_break + total > limit {
            return core::ptr::null_mut();
        }

        let block = sb.heap_break as *mut BlockHeader;
        unsafe {
            (*block).size = aligned;
            (*block).magic = BLOCK_MAGIC;
            (*block).next = core::ptr::null_mut();
        }
        sb.heap_break += total;
        unsafe { (block as *mut u8).add(core::mem::size_of::<BlockHeader>()) }
    })
    .unwrap_or(core::ptr::null_mut())
}

/// 归还 arena 内的堆块。
fn sandbox_free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    with(|sb| {
        let block = unsafe { ptr.sub(core::mem::size_of::<BlockHeader>()) } as *mut BlockHeader;
        // SAFETY: 只有本函数写过块头，先验魔数再动它。
        unsafe {
            if (*block).magic != BLOCK_MAGIC {
                return;
            }
            (*block).magic = 0;
            (*block).next = sb.free_list;
            sb.free_list = block;
        }
    });
}

/// 调整堆断点，返回旧断点。
fn sandbox_sbrk(increment: i32) -> i32 {
    with(|sb| {
        let old = sb.heap_break as u32;
        let limit = sb.arena.as_ptr() as usize + sb.arena.len() - ARENA_TAIL_RESERVE;
        let new = sb.heap_break.wrapping_add_signed(increment as isize);
        if new > limit {
            return -1;
        }
        sb.heap_break = new;
        old as i32
    })
    .unwrap_or(-1)
}

/// 整数平方根（牛顿迭代）。
fn isqrt(x: u32) -> u32 {
    if x < 2 {
        return x;
    }
    let mut g = x / 2;
    while g.saturating_mul(g) > x || (g + 1).saturating_mul(g + 1) <= x {
        g = (g + x / g) / 2;
        if g == 0 {
            break;
        }
    }
    g
}

/// 0..90 度整数正弦表，值为 `sin(deg) * 10000`。
const SIN_TABLE: [i32; 91] = [
    0, 175, 349, 523, 698, 872, 1045, 1219, 1392, 1564, 1736, 1908, 2079, 2250, 2419, 2588,
    2756, 2924, 3090, 3256, 3420, 3584, 3746, 3907, 4067, 4226, 4384, 4540, 4695, 4848, 5000,
    5150, 5299, 5446, 5592, 5736, 5878, 6018, 6157, 6293, 6428, 6561, 6691, 6820, 6947, 7071,
    7193, 7314, 7431, 7547, 7660, 7771, 7880, 7986, 8090, 8192, 8290, 8387, 8480, 8572, 8660,
    8746, 8829, 8910, 8988, 9063, 9135, 9205, 9272, 9336, 9397, 9455, 9511, 9563, 9613, 9659,
    9703, 9744, 9781, 9816, 9848, 9877, 9903, 9925, 9945, 9962, 9976, 9986, 9994, 9998, 10000,
];

/// 定点正弦，返回 `sin(deg) * 10000`。
///
/// 用 1 度查表 + 线性插值，误差 < 1/10000；no_std 下没有 `f32::sin`。
fn sin_deg(deg: i32) -> i32 {
    let d = deg.rem_euclid(360);
    let (sign, quad) = match d {
        0..=90 => (1, d),
        91..=180 => (1, 180 - d),
        181..=270 => (-1, d - 180),
        _ => (-1, 360 - d),
    };
    sign * SIN_TABLE[quad as usize]
}

/// 定点余弦，返回 `cos(deg) * 10000`。
fn cos_deg(deg: i32) -> i32 {
    sin_deg(deg + 90)
}

/// 软件 SHA-256，避免在 trap 上下文里抢 SHA 外设的锁。
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut msg = Vec::with_capacity(data.len() + 72);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for t in 0..16 {
            w[t] = u32::from_be_bytes([
                chunk[t * 4],
                chunk[t * 4 + 1],
                chunk[t * 4 + 2],
                chunk[t * 4 + 3],
            ]);
        }
        for t in 16..64 {
            let s0 = w[t - 15].rotate_right(7) ^ w[t - 15].rotate_right(18) ^ (w[t - 15] >> 3);
            let s1 = w[t - 2].rotate_right(17) ^ w[t - 2].rotate_right(19) ^ (w[t - 2] >> 10);
            w[t] = w[t - 16]
                .wrapping_add(s0)
                .wrapping_add(w[t - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d) = (h[0], h[1], h[2], h[3]);
        let (mut e, mut f, mut g, mut hh) = (h[4], h[5], h[6], h[7]);

        for t in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[t])
                .wrapping_add(w[t]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}
