//! 进程管理器与 4 KB 页池分配器。

use alloc::{format, string::String, vec, vec::Vec};
use core::cell::RefCell;

use critical_section::Mutex;

use crate::{
    fs,
    hal::console,
    nvram,
    sandbox, zc6,
};

/// 进程表容量。
pub const MAX_PROCS: usize = 8;
/// 页大小。
pub const PAGE_SIZE: usize = 4 * 1024;
/// 用户栈默认预留。
pub const DEFAULT_STACK: usize = 6 * 1024;
/// 最小堆预留。
pub const MIN_HEAP: usize = 4 * 1024;
/// 页池在 arena 里占用的字节数。
const POOL_SIZE: usize = 256 * 1024;

/// 进程生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum State {
    /// 槽位空闲。
    Free,
    /// 前台运行。
    RunningFg,
    /// 后台运行。
    RunningBg,
    /// 已挂起，内存仍占用。
    Suspended,
}

impl State {
    /// 表格里显示的短名。
    fn label(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::RunningFg => "run-fg",
            Self::RunningBg => "run-bg",
            Self::Suspended => "susp",
        }
    }
}

/// 进程控制块。
#[derive(Debug, Clone, defmt::Format)]
pub struct Proc {
    /// 进程 ID。
    pub pid: u16,
    /// 可执行文件名。
    pub name: String,
    /// 当前状态。
    pub state: State,
    /// 分配到的连续页数。
    pub pages: usize,
    /// 镜像字节数。
    pub bin_size: usize,
    /// 启动时刻（毫秒）。
    pub start_ms: u64,
    /// 采样到的 CPU 占用百分比。
    pub cpu_pct: u32,
    /// 退出码。
    pub exit_code: i32,
    /// 可执行文件在文件系统里的节点 ID，后台进程转前台时按它重新读镜像。
    pub file_id: u16,
}

/// 页池统计。
#[derive(Debug, Clone, Copy, defmt::Format)]
pub struct MemStats {
    /// 页池总字节数。
    pub total: usize,
    /// 已分配字节数。
    pub used: usize,
    /// 空闲字节数。
    pub free: usize,
    /// 最大连续空闲块字节数。
    pub largest_free: usize,
    /// 总页数。
    pub total_pages: usize,
    /// 空闲页数。
    pub free_pages: usize,
}

struct Manager {
    table: Vec<Proc>,
    pages: Vec<bool>,
    next_pid: u16,
    foreground: u16,
}

static MANAGER: Mutex<RefCell<Option<Manager>>> = Mutex::new(RefCell::new(None));

fn with<T>(f: impl FnOnce(&mut Manager) -> T) -> Option<T> {
    critical_section::with(|cs| {
        let mut guard = MANAGER.borrow_ref_mut(cs);
        guard.as_mut().map(f)
    })
}

/// 初始化进程表与页池。
pub fn init() {
    let pages = POOL_SIZE / PAGE_SIZE;
    let mgr = Manager {
        table: vec![
            Proc {
                pid: 0,
                name: String::new(),
                state: State::Free,
                pages: 0,
                bin_size: 0,
                start_ms: 0,
                cpu_pct: 0,
                exit_code: 0,
                file_id: 0,
            };
            MAX_PROCS
        ],
        pages: vec![false; pages],
        next_pid: 1,
        foreground: 0,
    };
    critical_section::with(|cs| *MANAGER.borrow_ref_mut(cs) = Some(mgr));
    defmt::info!("[PROC] 页池就绪: {} 页 x 4 KB", pages);
}

/// 从页池里分配连续页，返回起始页号。
fn alloc_pages(mgr: &mut Manager, count: usize) -> Option<usize> {
    if count == 0 || count > mgr.pages.len() {
        return None;
    }
    // Next-fit：从上次分配位置往后找第一段足够长的连续空闲页
    let n = mgr.pages.len();
    let mut run = 0;
    let mut start = 0;
    for i in 0..n {
        if mgr.pages[i] {
            run = 0;
            start = i + 1;
        } else {
            run += 1;
            if run >= count {
                for slot in &mut mgr.pages[start..start + count] {
                    *slot = true;
                }
                return Some(start);
            }
        }
    }
    None
}

/// 归还连续页。
fn free_pages(mgr: &mut Manager, start: usize, count: usize) {
    for slot in mgr.pages.iter_mut().skip(start).take(count) {
        *slot = false;
    }
}

/// 页池统计。
pub fn mem_stats() -> MemStats {
    with(|mgr| {
        let total_pages = mgr.pages.len();
        let free_pages = mgr.pages.iter().filter(|p| !**p).count();
        let mut largest = 0;
        let mut run = 0;
        for used in &mgr.pages {
            if *used {
                run = 0;
            } else {
                run += 1;
                largest = largest.max(run);
            }
        }
        MemStats {
            total: total_pages * PAGE_SIZE,
            used: (total_pages - free_pages) * PAGE_SIZE,
            free: free_pages * PAGE_SIZE,
            largest_free: largest * PAGE_SIZE,
            total_pages,
            free_pages,
        }
    })
    .unwrap_or(MemStats {
        total: 0,
        used: 0,
        free: 0,
        largest_free: 0,
        total_pages: 0,
        free_pages: 0,
    })
}

/// 读入可执行文件，必要时透明解压 `.zc6`。
fn load_image(id: u16) -> Result<Vec<u8>, &'static str> {
    let raw = fs::read_whole(id).ok_or("读文件失败")?;
    let zc6_ok = zc6::Header::parse(&raw);
    defmt::info!(
        "[PROC] 读入 {} B, 头 {=[u8]:x}, zc6={}",
        raw.len(),
        &raw[..raw.len().min(4)],
        zc6_ok.is_some()
    );
    if nvram::get().zc6_swap_mode.flash_enabled() {
        if let Some(hdr) = zc6_ok {
            let mut out = vec![0u8; hdr.raw_size as usize];
            return match zc6::decompress(&raw, &mut out) {
                Some(n) => {
                    out.truncate(n);
                    Ok(out)
                }
                None => Err("ZC6 解压失败"),
            };
        }
    }
    Ok(raw)
}

/// 把镜像装进 arena 并进入 U-Mode 执行，返回后收尾。
fn enter(pid: u16, path: &str, image: &[u8]) {
    if !sandbox::init(sandbox::DEFAULT_KERNEL_RESERVE) {
        finish(pid, -4);
        console::print("sandbox arena 初始化失败\r\n");
        return;
    }
    if !sandbox::load_payload(image) {
        sandbox::deinit();
        finish(pid, -1);
        console::print("镜像超过 arena 容量\r\n");
        return;
    }

    crate::led::set_aura_mode(nvram::AuraMode::Disabled);
    crate::led::set_color(0, 0, 0);
    console::print(&format!(
        "launching {} ({} B) in U-Mode sandbox...\r\n",
        path,
        image.len()
    ));

    let status = sandbox::run().unwrap_or(sandbox::Status::ErrNoMem);
    sandbox::deinit();
    crate::led::apply_aura_from_nvram();
    finish(pid, status as i32);
    console::print(&format!("sandbox session ended: {:?}\r\n", status));
}

/// 启动一个 payload。
///
/// shortcut: 单 hart 上真正的并发需要抢占式调度（trap 路径要跨任务保存 `mscratch`），
/// 这里做成协作式作业控制：`bg` 只登记进程，`fg <pid>` 才真正进入 U-Mode。
pub fn spawn(path: &str, cwd: u16, background: bool) -> Result<u16, &'static str> {
    if with(|mgr| mgr.foreground != 0).unwrap_or(false) {
        return Err("已有前台进程在跑");
    }

    let id = fs::resolve(path, cwd).ok_or("找不到文件")?;
    if fs::get_type(id) != fs::TYPE_FILE {
        return Err("不是文件");
    }
    let image = load_image(id)?;

    let needed_pages = (image.len() + DEFAULT_STACK + MIN_HEAP).div_ceil(PAGE_SIZE);
    let state = if background {
        State::RunningBg
    } else {
        State::RunningFg
    };
    let Some(pid) = with(|mgr| -> Option<u16> {
        alloc_pages(mgr, needed_pages)?;
        let slot = mgr.table.iter().position(|p| p.state == State::Free)?;
        let pid = mgr.next_pid;
        mgr.next_pid += 1;
        mgr.table[slot] = Proc {
            pid,
            name: String::from(path.rsplit('/').next().unwrap_or(path)),
            state,
            pages: needed_pages,
            bin_size: image.len(),
            start_ms: embassy_time::Instant::now().as_millis(),
            cpu_pct: 0,
            exit_code: 0,
            file_id: id,
        };
        if !background {
            mgr.foreground = pid;
        }
        Some(pid)
    })
    .flatten()
    else {
        return Err("没有足够的连续空闲页");
    };

    if background {
        console::print(&format!(
            "[JOB {}] 已登记（单 hart 无抢占，`fg {}` 才真正执行）\r\n",
            pid, pid
        ));
        return Ok(pid);
    }

    enter(pid, path, &image);
    Ok(pid)
}

/// 把已登记的进程拉到前台并执行。
pub fn run_registered(pid: u16) -> Result<(), &'static str> {
    if with(|mgr| mgr.foreground != 0).unwrap_or(false) {
        return Err("已有前台进程在跑");
    }

    let Some((name, file_id)) = with(|mgr| {
        let slot = mgr.table.iter().position(|p| p.pid == pid)?;
        let p = &mgr.table[slot];
        if p.state == State::Free {
            return None;
        }
        Some((p.name.clone(), p.file_id))
    })
    .flatten()
    else {
        return Err("no such pid");
    };

    with(|mgr| {
        let slot = mgr.table.iter().position(|p| p.pid == pid)?;
        mgr.table[slot].state = State::RunningFg;
        mgr.foreground = pid;
        Some(())
    })
    .flatten()
    .ok_or("no such pid")?;

    let image = load_image(file_id)?;
    enter(pid, &name, &image);
    Ok(())
}

/// 收尾：清前台标记、还页、释放槽位。
fn finish(pid: u16, exit_code: i32) {
    with(|mgr| {
        if let Some(slot) = mgr.table.iter().position(|p| p.pid == pid) {
            let pages = mgr.table[slot].pages;
            mgr.table[slot].state = State::Free;
            mgr.table[slot].exit_code = exit_code;
            mgr.table[slot].pages = 0;
            mgr.table[slot].name.clear();
            if mgr.foreground == pid {
                mgr.foreground = 0;
            }
            // 页池是线性编号的，归还时按页数从池尾回退即可
            let start = mgr.pages.len().saturating_sub(pages);
            free_pages(mgr, start, pages);
        }
    });
}

/// 终止进程。
pub fn kill(pid: u16) -> Result<(), &'static str> {
    let exists = with(|mgr| mgr.table.iter().any(|p| p.pid == pid && p.state != State::Free))
        .unwrap_or(false);
    if !exists {
        return Err("no such pid");
    }
    finish(pid, 0);
    Ok(())
}

/// 挂起指定进程。
pub fn suspend(pid: u16) -> Result<(), &'static str> {
    with(|mgr| {
        if let Some(slot) = mgr.table.iter().position(|p| p.pid == pid) {
            mgr.table[slot].state = State::Suspended;
            Ok(())
        } else {
            Err("no such pid")
        }
    })
    .unwrap_or(Err("no such pid"))
}

/// 挂起当前前台进程。
pub fn suspend_foreground() -> Result<(), &'static str> {
    let pid = with(|mgr| mgr.foreground).unwrap_or(0);
    if pid == 0 {
        return Err("没有前台进程");
    }
    suspend(pid)
}

/// 把进程恢复到后台。
pub fn resume_background(pid: u16) -> Result<(), &'static str> {
    with(|mgr| {
        if let Some(slot) = mgr.table.iter().position(|p| p.pid == pid) {
            mgr.table[slot].state = State::RunningBg;
            Ok(())
        } else {
            Err("no such pid")
        }
    })
    .unwrap_or(Err("no such pid"))
}

/// 当前前台进程 ID，0 表示 shell 持有控制台。
/// 打印进程表与页池状态。
pub fn print_top(emit: &mut impl FnMut(&str)) {
    let stats = mem_stats();
    emit("PID  STATE      PAGES  SIZE    NAME\r\n");
    emit("--------------------------------------------------\r\n");
    let procs: Vec<Proc> = with(|mgr| mgr.table.clone()).unwrap_or_default();
    for p in procs.iter().filter(|p| p.state != State::Free) {
        emit(&format!(
            "{:<4} {:<10} {:<6} {:<7} {}\r\n",
            p.pid, p.state.label(), p.pages, p.bin_size, p.name
        ));
    }
    emit(&format!(
        "\r\npage pool: {} / {} pages free ({} B), largest block {} B\r\n",
        stats.free_pages, stats.total_pages, stats.free, stats.largest_free
    ));
}

/// 从控制台接收一个二进制 payload 并存进文件系统。
///
/// 传输协议与上游 `openc6_loader` 一致：裸字节流，发完就停，设备侧靠静默判结束。
/// 不能用 `0x04` 之类的结束符——二进制 payload 里出现任何字节都是正常的。
/// `0x03`（Ctrl+C）只在还没收到数据时才当中止，避免误伤二进制内容。
pub fn serial_receive(path: &str) -> Result<usize, &'static str> {
    /// 中止标记，仅在尚未收到数据时生效。
    const ABORT: u8 = 0x03;
    /// 判定发送结束的静默时长。
    const SILENCE_MS: u64 = 750;

    console::print("waiting for payload (send raw bytes, stop to finish)...
");

    let mut buf = Vec::new();
    let mut last_rx = esp_hal::time::Instant::now();

    loop {
        if console::has_data() {
            let b = console::getc();
            if b == ABORT && buf.is_empty() {
                return Err("aborted");
            }
            buf.push(b);
            last_rx = esp_hal::time::Instant::now();
        } else {
            crate::me::pet_watchdog();
            if !buf.is_empty()
                && esp_hal::time::Instant::now() - last_rx
                    > esp_hal::time::Duration::from_millis(SILENCE_MS)
            {
                break;
            }
        }
    }

    if buf.is_empty() {
        return Err("no data received");
    }

    let parent = match fs::find_id("downloaded", 0) {
        Some(id) => id,
        None => fs::mkdir("downloaded", 0).ok_or("无法创建 /downloaded")?,
    };
    let name = path.rsplit('/').next().unwrap_or("payload.bin");
    fs::write_file(name, &buf, parent).ok_or("写入文件系统失败")?;
    Ok(buf.len())
}

