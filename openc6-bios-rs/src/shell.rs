//! Micro UNIX Shell：交互式命令行与进程作业控制。

use alloc::{format, string::String, vec::Vec};
use embassy_executor::Spawner;

use crate::{
    fs,
    hal::{clock, console, power},
    me, nvram, sandbox,
};

/// 单条命令的最大长度。
const LINE_MAX: usize = 128;

/// shell 会话状态。
struct Shell {
    /// 当前工作目录节点 ID。
    cwd: u16,
    /// 执行器句柄，`setup` 这类需要起后台任务的命令用。
    spawner: Option<Spawner>,
}

/// 执行一条命令并把输出写进返回的字符串（供 Web Shell 复用）。
pub fn execute_capture(line: &str) -> String {
    let mut out = String::new();
    run(
        line,
        &mut Shell {
            cwd: 0,
            spawner: None,
        },
        Some(&mut out),
    );
    out
}

/// 跑交互式 shell，直到用户 `poweroff`。
///
/// 每轮让出一次调度器，不把 embassy 执行器堵死。
pub async fn repl(spawner: Spawner) -> ! {
    let mut sh = Shell {
        cwd: 0,
        spawner: Some(spawner),
    };
    console::print("\r\nOpenC6 Micro UNIX Shell. 输入 help 查看命令。\r\n");

    // 启动提示是在 REPL 之前打出来的，UART0 会把它们收回来当成第一条命令。
    // 进入输入循环前把残留字节排空。
    while console::has_data() {
        let _ = console::getc();
    }

    let mut line = String::new();
    loop {
        prompt(sh.cwd);
        line.clear();

        loop {
            if !console::has_data() {
                me::pet_watchdog();
                me::poll_lp_requests();
                me::note_idle();
                embassy_time::Timer::after(embassy_time::Duration::from_millis(5)).await;
                continue;
            }

            let c = console::getc();
            match c {
                b'\r' | b'\n' => {
                    console::print("\r\n");
                    if !line.is_empty() {
                        // `wifi scan` / `pxe` 要 await，同步的 run() 分发不了，单独拦
                        if line.trim() == "wifi scan" {
                            crate::net::scan_print().await;
                        } else if let Some(url) = line.trim().strip_prefix("pxe ") {
                            console::print(&format!("downloading {url} ...\r\n"));
                            match crate::pxe::download_payload(url.trim()).await {
                                Ok(n) => console::print(&format!(
                                    "{n} bytes stored in /downloaded/payload.bin\r\n"
                                )),
                                Err(e) => console::print(&format!("pxe failed: {e}\r\n")),
                            }
                        } else {
                            me::note_busy();
                            run(&line, &mut sh, None);
                        }
                    }
                    break;
                }
                0x08 | 0x7F => {
                    if line.pop().is_some() {
                        console::print("\x08 \x08");
                    }
                }
                0x18 => {
                    // Ctrl+X：挂起前台进程
                    console::print("^X\r\n");
                    break;
                }
                c if c.is_ascii_graphic() || c == b' ' => {
                    if line.len() < LINE_MAX - 1 {
                        line.push(c as char);
                        console::putc(c);
                    }
                }
                _ => {}
            }
        }
    }
}

fn prompt(cwd: u16) {
    let dir = if cwd == 0 {
        String::from("/")
    } else {
        format!("/{}", fs::name_of(cwd))
    };
    console::print(&format!("openc6_fs [{dir}] /> "));
}

/// 执行一条命令行；`capture` 有值时输出同时写进它。
fn run(line: &str, sh: &mut Shell, mut capture: Option<&mut String>) {
    let args: Vec<&str> = line.split_whitespace().collect();
    let Some(&cmd) = args.first() else { return };

    let mut emit = |s: &str| {
        console::print(s);
        if let Some(out) = capture.as_deref_mut() {
            out.push_str(s);
        }
    };

    match cmd {
        "help" => emit(
            "help                          本命令表\r\n\
             info                          硬件 / 调频 / 温度 / ME 状态\r\n\
             mem                           内存与 arena 用量\r\n\
             top                           进程表与页池状态\r\n\
             reboot                        热重启\r\n\
             poweroff [秒] | exit          进 S5 软关机，带秒数则定时自动唤醒\r\n\
             boot <path> [bg]              在 U-Mode 沙箱里跑 payload\r\n\
             kill <pid>                    终止进程\r\n\
             suspend [pid]                 挂起进程（ZSWAP 压缩）\r\n\
             fg <pid> / bg <pid>           前后台切换\r\n\
             serial [path]                 从 USB 接收 payload\r\n\
             pxe <url>                     从网络下载 payload\r\n\
             wifi scan|connect|status|disconnect\r\n\
             ls [path] / cd <path> / mkdir <path>\r\n\
             cat <path> / write <path> <text>\r\n\
             cp <src> <dst> / mv <src> <dst> / rm <path>\r\n\
             setup                         起 BIOS Setup 的 AP + Web UI\r\n\
             format                        格式化 openc6_fs 分区\r\n\
             wdtest [秒]                   停止喂狗，验证 LP 看门狗复位\r\n",
        ),
        "info" => {
            let cfg = nvram::get();
            emit(&format!(
                "CPU            : {} MHz (LP 目标 {} MHz)\r\n\
                 Governor       : {:?}\r\n\
                 CPU load       : {} %\r\n\
                 Temp           : {} C\r\n\
                 ME             : {}\r\n\
                 LP heartbeat   : {}\r\n\
                 Free heap      : {} B\r\n\
                 Arena          : {} B\r\n\
                 ZC6 swap mode  : {:?}\r\n\
                 Wi-Fi          : {} ({})\r\n",
                clock::get_cpu_mhz(),
                me::lp_target_freq(),
                cfg.cpu_governor,
                me::lp_cpu_load(),
                me::temperature_c(),
                if me::is_active() { "ACTIVE" } else { "OFF" },
                me::lp_heartbeat(),
                esp_alloc::HEAP.free(),
                sandbox::arena_size(),
                cfg.zc6_swap_mode,
                crate::net::ip_string(),
                if crate::net::is_connected() {
                    "STA+AP connected"
                } else if crate::net::sta_present() {
                    "STA+AP down"
                } else {
                    "AP beaconing"
                },
            ));
        }
        "wdtest" => {
            // 只关掉喂狗，不阻塞执行器：同步空转或 `boot spin.bin` 都会卡死
            // 执行器线程，让 esp-rtos 自己的看门狗抢先复位，掩盖 LP 看门狗。
            let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(25);
            me::suppress_petting(secs);
            emit(&format!(
                "已停止喂狗 {secs} 秒；LP 看门狗阈值 15 秒，超时应复位芯片\r\n"
            ));
        }
        "mem" => emit(&format!(
            "Heap total : {} B\r\nHeap free  : {} B\r\nArena size : {} B\r\n",
            esp_alloc::HEAP.used() + esp_alloc::HEAP.free(),
            esp_alloc::HEAP.free(),
            sandbox::arena_size(),
        )),
        "top" => {
            crate::proc::print_top(&mut emit);
        }
        "reboot" => {
            emit("rebooting...\r\n");
            power::reboot();
        }
        "poweroff" | "exit" => {
            crate::led::set_aura_mode(nvram::AuraMode::Disabled);
            crate::led::set_color(0, 0, 0);
            // 带秒数就挂定时唤醒，测试时不会把板子睡死
            match args.get(1).and_then(|s| s.parse::<u64>().ok()) {
                Some(secs) => {
                    emit(&format!("entering S5, auto-wake in {secs}s...\r\n"));
                    power::enter_s5_with_timer(secs.max(2));
                }
                None => {
                    emit("entering S5...\r\n");
                    power::enter_s5();
                }
            }
        }
        "boot" => match args.get(1) {
            Some(path) => {
                let bg = args.iter().any(|a| *a == "bg" || *a == "&");
                match crate::proc::spawn(path, sh.cwd, bg) {
                    Ok(pid) => emit(&format!("pid {pid} started\r\n")),
                    Err(e) => emit(&format!("boot failed: {e}\r\n")),
                }
            }
            None => emit("usage: boot <path> [bg]\r\n"),
        },
        "kill" => {
            if let Some(pid) = args.get(1).and_then(|s| s.parse::<u16>().ok()) {
                let msg = if crate::proc::kill(pid).is_ok() {
                    format!("pid {pid} killed\r\n")
                } else {
                    format!("no such pid: {pid}\r\n")
                };
                emit(&msg);
            } else {
                emit("usage: kill <pid>\r\n");
            }
        }
        "suspend" => {
            let r = match args.get(1).and_then(|s| s.parse::<u16>().ok()) {
                Some(pid) => crate::proc::suspend(pid),
                None => crate::proc::suspend_foreground(),
            };
            emit(&format!("suspend: {r:?}\r\n"));
        }
        "fg" => {
            if let Some(pid) = args.get(1).and_then(|s| s.parse::<u16>().ok()) {
                // fg 真正进入 U-Mode 执行，而不是只改状态位
                match crate::proc::run_registered(pid) {
                    Ok(()) => emit("fg: 已结束\r\n"),
                    Err(e) => emit(&format!("fg: {e}\r\n")),
                }
            } else {
                emit("usage: fg <pid>\r\n");
            }
        }
        "bg" => {
            if let Some(pid) = args.get(1).and_then(|s| s.parse::<u16>().ok()) {
                let r = crate::proc::resume_background(pid);
                emit(&format!("bg: {r:?}\r\n"));
            } else {
                emit("usage: bg <pid>\r\n");
            }
        }
        "serial" => {
            let path = args.get(1).copied().unwrap_or("/downloaded/payload.bin");
            emit(&format!("receiving into {path} ...\r\n"));
            match crate::proc::serial_receive(path) {
                Ok(n) => emit(&format!("{n} bytes stored\r\n")),
                Err(e) => emit(&format!("serial failed: {e}\r\n")),
            }
        }
        // 真正的下载在 repl 里异步执行；这里只兜底提示，避免同步路径阻塞执行器
        "pxe" => emit("pxe 需要在交互式 shell 里执行\r\n"),
        "wifi" => match args.get(1).copied() {
            Some("status") => {
                emit(&format!(
                    "mode   : {}\r\nap ip  : {}\r\nsta ip : {}\r\nstate  : {}\r\n",
                    if crate::net::sta_present() { "STA+AP" } else { "AP" },
                    crate::net::ip_string(),
                    crate::net::sta_ip_string(),
                    if crate::net::is_connected() {
                        "connected"
                    } else {
                        "AP beaconing"
                    },
                ));
            }
            Some("connect") => {
                let ssid = args.get(2).copied().unwrap_or("");
                let pass = args.get(3).copied().unwrap_or("");
                nvram::update(|c| c.set_wifi_sta(ssid, pass));
                emit("credentials saved, reconnecting...\r\n");
                // 开机时没有凭据就不会初始化 Wi-Fi，这里按需补上，否则控制器是 None
                match sh.spawner {
                    Some(spawner) => {
                        if crate::net::init(spawner).is_none() {
                            emit("Wi-Fi 初始化失败\r\n");
                        } else if !crate::net::ensure_sta(spawner) {
                            emit("STA 接口创建失败\r\n");
                        } else {
                            emit(&format!("start_sta: {}\r\n", crate::net::start_sta()));
                        }
                    }
                    None => emit("wifi connect 只在交互式 shell 里可用\r\n"),
                }
            }
            Some("disconnect") => {
                nvram::update(|c| c.set_wifi_sta("", ""));
                crate::net::stop_ap();
                emit("credentials cleared\r\n");
            }
            Some("scan") => emit("scan: 由 STA 驱动异步进行，暂未接入命令输出\r\n"),
            _ => emit("usage: wifi scan|connect <ssid> [pass]|status|disconnect\r\n"),
        },
        "ls" => {
            let (dir, _) = resolve_dir(args.get(1).copied(), sh.cwd, &mut emit);
            emit("ID    TYPE NAME             SIZE\r\n");
            emit("----------------------------------------\r\n");
            let mut found = 0;
            for node in fs::list_dir(dir) {
                emit(&format!(
                    "{:<5} {:<4} {:<16} {}\r\n",
                    node.id,
                    if node.type_ == fs::TYPE_DIR { "DIR" } else { "FILE" },
                    node.name_str(),
                    node.size
                ));
                found += 1;
            }
            if found == 0 {
                emit("  (empty directory)\r\n");
            }
        }
        "cd" => {
            let Some(target) = args.get(1) else {
                emit("usage: cd <path>\r\n");
                return;
            };
            match fs::resolve(target, sh.cwd) {
                Some(id) if fs::get_type(id) == fs::TYPE_DIR => sh.cwd = id,
                Some(_) => emit("not a directory\r\n"),
                None => emit("no such directory\r\n"),
            }
        }
        "mkdir" => {
            let Some(name) = args.get(1) else {
                emit("usage: mkdir <name>\r\n");
                return;
            };
            match fs::split_path(name, sh.cwd) {
                Some((parent, base)) => match fs::mkdir(base, parent) {
                    Some(id) => emit(&format!("created dir id {id}\r\n")),
                    None => emit("mkdir failed\r\n"),
                },
                None => emit("bad path\r\n"),
            }
        }
        "cat" => {
            let Some(name) = args.get(1) else {
                emit("usage: cat <path>\r\n");
                return;
            };
            match fs::resolve(name, sh.cwd).and_then(fs::read_whole) {
                Some(data) => {
                    let text = String::from_utf8_lossy(&data).into_owned();
                    emit(&text);
                    emit("\r\n");
                }
                None => emit("no such file\r\n"),
            }
        }
        "write" => {
            let (Some(name), Some(text)) = (args.get(1), args.get(2)) else {
                emit("usage: write <path> <text>\r\n");
                return;
            };
            let body: String = args[2..].join(" ");
            let _ = text;
            match fs::split_path(name, sh.cwd) {
                Some((parent, base)) => match fs::write_file(base, body.as_bytes(), parent) {
                    Some(id) => emit(&format!("wrote {} B as id {id}\r\n", body.len())),
                    None => emit("write failed\r\n"),
                },
                None => emit("bad path\r\n"),
            }
        }
        "cp" => {
            let (Some(src), Some(dst)) = (args.get(1), args.get(2)) else {
                emit("usage: cp <src> <dst>\r\n");
                return;
            };
            let Some(data) = fs::resolve(src, sh.cwd).and_then(fs::read_whole) else {
                emit("no such source\r\n");
                return;
            };
            match fs::split_path(dst, sh.cwd) {
                Some((parent, base)) => match fs::write_file(base, &data, parent) {
                    Some(_) => emit("copied\r\n"),
                    None => emit("copy failed\r\n"),
                },
                None => emit("bad path\r\n"),
            }
        }
        "mv" => {
            let (Some(src), Some(dst)) = (args.get(1), args.get(2)) else {
                emit("usage: mv <src> <dst>\r\n");
                return;
            };
            let Some(data) = fs::resolve(src, sh.cwd).and_then(fs::read_whole) else {
                emit("no such source\r\n");
                return;
            };
            let Some((parent, base)) = fs::split_path(dst, sh.cwd) else {
                emit("bad path\r\n");
                return;
            };
            let Some(new_id) = fs::write_file(base, &data, parent) else {
                emit("move failed\r\n");
                return;
            };
            if let Some(old_id) = fs::resolve(src, sh.cwd) {
                fs::delete(old_id);
            }
            emit(&format!("moved as id {new_id}\r\n"));
        }
        "rm" => {
            let Some(name) = args.get(1) else {
                emit("usage: rm <path>\r\n");
                return;
            };
            // 走完整路径解析，和 cat / cp / mv 保持一致
            match fs::resolve(name, sh.cwd) {
                Some(id) => {
                    fs::delete(id);
                    emit("removed\r\n");
                }
                None => emit("no such entry\r\n"),
            }
        }
        "setup" => match sh.spawner {
            Some(spawner) => {
                if crate::net::init(spawner).is_none() {
                    emit("Wi-Fi 初始化失败\r\n");
                } else if crate::net::start_ap() {
                    emit(&format!(
                        "AP `{}` 已启动，密码 `{}`\r\n浏览 http://{}/\r\n",
                        crate::BIOS_AP_SSID,
                        crate::BIOS_AP_PASS,
                        crate::net::AP_GATEWAY
                    ));
                } else {
                    emit("AP 启动失败\r\n");
                }
            }
            None => emit("setup 只在交互式 shell 里可用\r\n"),
        },
        "format" => {
            fs::format();
            sh.cwd = 0;
            emit("format complete\r\n");
        }
        other => emit(&format!("unknown command: {other}\r\n")),
    }
}

/// 解析目录参数，返回 `(目录 ID, 是否成功)`。
fn resolve_dir(arg: Option<&str>, cwd: u16, emit: &mut impl FnMut(&str)) -> (u16, bool) {
    match arg {
        // 不带参数就用当前工作目录，别硬编码成根目录
        None => (cwd, true),
        Some("/") => (0, true),
        Some(path) => match fs::resolve(path, cwd) {
            Some(id) if fs::get_type(id) == fs::TYPE_DIR => (id, true),
            Some(_) => {
                emit("not a directory\r\n");
                (cwd, false)
            }
            None => {
                emit("no such directory\r\n");
                (cwd, false)
            }
        },
    }
}
