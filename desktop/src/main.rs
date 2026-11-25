//! 副屏上位机（命令行）：抓屏线程缩放 + 分块 diff 后把整帧发布到槽位，
//! 每个串口一条发送线程用原子游标逐块抢活并行推流；数据不等应答（板子对数据
//! 不回 ACK，丢了就丢了，画面靠后续帧补），整帧的块都写完就发布下一帧。
//! 板子的状态（帧率/忙碌毫秒）只从控制命令（ping）的应答里拿。

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use clap::Parser;
use screen_capture::CaptureStreamConfig;

mod frame;
mod link;

use frame::{Frame, HEIGHT, TILE, Tile, WIDTH};
use link::{Link, LinkError, Report};

/// 默认帧率（初始目标，之后按板子上报的能力动态调整）。
const DEFAULT_FPS: f64 = 10.0;
/// 帧率上限，与固件一致。
const MAX_FPS: f64 = 30.0;
/// 帧率下限。
const MIN_FPS: f64 = 1.0;
/// 目标帧率用 0.1 fps 的定点数放在原子变量里，抓屏线程直接读。
const TARGET_SCALE: f64 = 10.0;
/// 调整帧率的最小间隔。
const ADJUST: Duration = Duration::from_millis(250);
/// 空闲时用控制命令查询板子帧率的间隔。
const PING: Duration = Duration::from_millis(500);
/// 状态行刷新间隔。
const REPORT: Duration = Duration::from_secs(1);
/// 一条消息最多几块 tile，与固件一致。
const MAX_BATCH: usize = 32;
/// 一整帧切成多少条消息：粗粒度摊薄每消息的帧头与系统调用，细粒度保延迟。
const MESSAGES_PER_FRAME: usize = 10;
/// ping 连续不通这么多次就判死：一次超时可能是被挤，连着就是链路断了。
const PING_STRIKE: u32 = 2;
/// 死口重开的最小间隔：重开 CH343 会经 DTR 复位整块板子、画面闪掉，绝不能频繁试。
const REVIVE_REOPEN: Duration = Duration::from_secs(5);
/// 打开所有串口后等板子复位与 USB 重枚举稳定：打开板载 CH343 会经 DTR/RTS
/// 把板子复位一次，原生 USB 口随之重枚举。
const SETTLE: Duration = Duration::from_millis(2500);
/// 默认波特率（USB CDC 链路会忽略；走 UART0 时要与固件一致）。
const DEFAULT_BAUD: u32 = 2_000_000;

/// 副屏上位机：抓 Wayland 输出的一角，按 16×16 分块 diff，走串口推给 ESP32-C6。
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// 列出可抓的输出与可用的串口后退出
    #[arg(long)]
    list: bool,

    /// 每帧打一行日志
    #[arg(short, long)]
    verbose: bool,

    /// 把鼠标指针抓进帧里
    #[arg(long)]
    cursor: bool,

    /// Wayland 输出名，缺省取第一个
    #[arg(long)]
    output: Option<String>,

    /// 串口设备列表，逗号分隔（也可重复给多次）；缺省自动探测所有 USB 串口。
    /// 所有口逐块并行抢发；挂掉的口自动退出（下一帧整屏补推）、恢复后自动归队
    #[arg(long, value_delimiter = ',')]
    serial: Vec<String>,

    /// 波特率（USB CDC 链路忽略；走 UART0 时要与固件一致）
    #[arg(long, default_value_t = DEFAULT_BAUD)]
    baud: u32,

    /// 初始目标帧率，运行中按板子上报的 busy_ms 自动调整（上限 30）
    #[arg(long, default_value_t = DEFAULT_FPS)]
    fps: f64,
}

fn main() {
    let args = Args::parse();

    if args.list {
        list();
        return;
    }

    if let Err(error) = run(args) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

/// 列出可抓的输出与可用的串口。
fn list() {
    match screen_capture_wayland_wlr::available_screens() {
        Ok(screens) => {
            println!("outputs:");
            for screen in screens {
                println!(
                    "  {} {}x{} scale {}",
                    screen.name,
                    screen.logical_size.width,
                    screen.logical_size.height,
                    screen.scale_factor
                );
            }
        }
        Err(error) => println!("outputs: {error}"),
    }

    println!("serial ports:");
    for port in serial_ports() {
        println!("  {port}");
    }
}

/// 系统里的 USB 串口。
fn usb_ports() -> Vec<String> {
    serial_ports()
        .into_iter()
        .filter(|name| name.contains("ttyACM") || name.contains("ttyUSB"))
        .collect()
}

/// 系统里的串口，USB 的排在主板自带的 `ttyS*` 前面。
fn serial_ports() -> Vec<String> {
    let mut ports: Vec<String> = serialport::available_ports()
        .map(|ports| ports.into_iter().map(|port| port.port_name).collect())
        .unwrap_or_default();
    ports.sort_by_key(|name| !name.contains("ttyACM") && !name.contains("ttyUSB"));
    ports
}

/// 口名转短标签，只用于状态行。
fn short_label(path: &str) -> String {
    if path.contains("1a86") {
        "ch343".into()
    } else if path.contains("Espressif") || path.contains("303a") {
        "usb-jtag".into()
    } else {
        path.rsplit('/').next().unwrap_or(path).to_string()
    }
}

/// 把 /dev/ttyACMx 这类会互换的名字换成 /dev/serial/by-id 的稳定路径（能对上才换），
/// 再把板载 CH343 排前面：开口的 DTR/RTS 会把板子复位一次，复位要发生在其它口打开之前。
fn resolve_ports(candidates: &[String]) -> Vec<String> {
    let by_id: Vec<(String, String)> = std::fs::read_dir("/dev/serial/by-id")
        .map(|entries| {
            entries
                .filter_map(|entry| {
                    let path = entry.ok()?.path();
                    let name = path.file_name()?.to_str()?.to_string();
                    let target = path.read_link().ok()?;
                    let base = target.file_name()?.to_str()?.to_string();
                    Some((base, name))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut ports: Vec<String> = candidates
        .iter()
        .map(|name| {
            let base = name.rsplit('/').next().unwrap_or(name);
            by_id
                .iter()
                .find(|(target, _)| target == base)
                .map(|(_, name)| format!("/dev/serial/by-id/{name}"))
                .unwrap_or_else(|| name.clone())
        })
        .collect();
    ports.sort_by_key(|port| !port.contains("1a86"));
    ports
}

/// 依次打开每个候选口；等板子复位与 USB 重枚举稳定后逐个 ping，
/// ping 不通的再开一次重试（fd 可能因重枚举失效）。返回所有能应答的口，一个都不应答才报错。
fn connect(candidates: &[String], baud: u32) -> Result<Vec<(String, Link)>, String> {
    let ports = resolve_ports(candidates);
    let mut opened: Vec<Option<Link>> = Vec::new();
    let mut errors = Vec::new();
    for port in &ports {
        match Link::open(port, baud) {
            Ok(link) => opened.push(Some(link)),
            Err(error) => {
                errors.push(format!("  {port}: {error}"));
                opened.push(None);
            }
        }
    }
    thread::sleep(SETTLE);
    let mut links = Vec::new();
    let mut silent = Vec::new();
    for (port, maybe) in ports.iter().zip(opened) {
        let mut answered = false;
        if let Some(mut link) = maybe
            && link.ping().is_ok()
        {
            links.push((port.clone(), link));
            answered = true;
        } else if let Ok(mut link) = Link::open(port, baud)
            && link.ping().is_ok()
        {
            links.push((port.clone(), link));
            answered = true;
        }
        if !answered {
            silent.push(port.clone());
        }
    }
    if links.is_empty() {
        errors.extend(silent.iter().map(|port| format!("  {port}: no reply")));
        return Err(format!("no board answered, tried:\n{}", errors.join("\n")));
    }
    if !silent.is_empty() {
        eprintln!("ignored silent ports: {}", silent.join(", "));
    }
    Ok(links)
}

/// 一帧待推的活：所有口共享，空闲口用原子游标抢批，速率快的口自然抢得多。
struct Job {
    /// 帧号，板子靠它丢过期数据。
    frame_no: u16,
    /// 整帧脏块。
    tiles: Arc<Vec<Tile>>,
    /// 下一批的批号。
    cursor: AtomicUsize,
    /// 本帧每条消息带几块。
    batch: usize,
    /// 本帧总批数。
    batches: usize,
    /// 已推完的批数，等于总批数时本帧推完。
    done: AtomicUsize,
    /// 所有块都已写完（数据不等应答），抓屏线程看到它才会发布下一帧。
    sent_all: AtomicBool,
    /// 本帧发布时刻。
    published: Instant,
}

/// 抓屏线程与各发送线程共享的状态。
struct Shared {
    /// 当前待推的整帧；没校验完就不会被替换。
    /// 待发帧队列：最多两帧（一个在发、一个备好），发线程取队首。
    /// 只淘汰「批已全被取完」的帧——发了一半的帧绝不能丢，否则那些块永久停旧。
    slot: Mutex<VecDeque<Arc<Job>>>,
    /// 目标帧率 × 10。
    target: AtomicU32,
    stats: Mutex<Stats>,
    /// 各口存活标志，下标即口序号；挂掉后不再抢活。
    alive: Vec<AtomicBool>,
    /// 板子忙碌毫秒的指数平滑。
    busy: Mutex<f64>,
    /// 帧号计数（发布时取号）。
    frame_no: AtomicU32,
    /// 有口挂掉后下一帧整屏重推，补上漏掉的块。
    refresh: AtomicBool,
    /// 上次调整目标帧率的时刻。
    last_adjust: Mutex<Option<Instant>>,
    /// 程序启动时刻，状态行算速率用。
    started: Instant,
    /// 各口累计块数/字节/在写里花的微秒，下标即口序号（状态行展示用）。
    port_batches: Vec<AtomicU64>,
    port_bytes: Vec<AtomicU64>,
    port_us: Vec<AtomicU64>,
    /// 下次打印状态行的时刻。
    next_report: Mutex<Option<Instant>>,
    /// 状态行里的口标签。
    label: String,
    /// 各口的短标签，下标即口序号。
    port_labels: Vec<String>,
}

/// 两个线程共同维护的统计。
#[derive(Default)]
struct Stats {
    /// 抓屏线程处理过的帧数（含被丢弃的）。
    captured: u64,
    /// 真正发出去的帧数。
    sent: u64,
    /// 累计发出的 tile 数。
    tiles: u64,
    /// 累计发出的载荷字节数（压缩后）。
    bytes: u64,
    /// 累计的原始像素字节数，用来看压缩比。
    raw_bytes: u64,
    /// 最近一帧的脏块数。
    dirty: usize,
    /// 板子上报的帧率。
    board_fps: u8,
    /// 板子渲染一帧的忙碌毫秒数。
    busy_ms: u16,
    /// 当前目标帧率。
    target: f64,
    /// 上一帧从发布到推完的耗时（微秒）。
    send_us: u64,
    /// 推送一帧的最长耗时（微秒），看链路峰值。
    send_max_us: u64,
    /// 上一次抓屏回调到现在的时间（微秒）。
    loop_us: u64,
    /// 库报告的抓屏耗时（微秒）。
    capture_us: u64,
    /// 出错后两个线程都停。
    error: Option<String>,
}

impl Shared {
    fn fail(&self, cancel: &AtomicBool, error: String) {
        if let Ok(mut stats) = self.stats.lock()
            && stats.error.is_none()
        {
            stats.error = Some(error);
        }
        cancel.store(true, Ordering::Relaxed);
    }

    /// 记下板子的上报值，并把忙碌时长做指数平滑。
    fn observe(&self, report: Report) {
        if let Ok(mut stats) = self.stats.lock() {
            stats.board_fps = report.fps;
            stats.busy_ms = report.busy_ms;
        }
        if let Ok(mut busy) = self.busy.lock() {
            *busy = match (*busy, u64::from(report.busy_ms)) {
                (_, 0) => *busy,
                (0.0, sample) => sample as f64,
                (avg, sample) => avg * 0.7 + (sample as f64) * 0.3,
            };
        }
    }
}

fn run(args: Args) -> Result<(), String> {
    let screens =
        screen_capture_wayland_wlr::available_screens().map_err(|error| error.to_string())?;
    let name = match &args.output {
        Some(name) => name.clone(),
        None => screens
            .first()
            .ok_or("no wayland output available")?
            .name
            .clone(),
    };

    // 指定了口就只用指定的那些，一个都不应答就报错——不偷偷换到别的口。
    // 只有完全没指定时才自动探测。
    let candidates = if args.serial.is_empty() {
        usb_ports()
    } else {
        args.serial.clone()
    };
    if candidates.is_empty() {
        return Err("no usb serial port found".into());
    }

    let links = connect(&candidates, args.baud)?;
    let labels: Vec<String> = links.iter().map(|(port, _)| short_label(port)).collect();
    println!(
        "output {name} -> {} @ {} baud, scale to {WIDTH}x{HEIGHT}, {} fps",
        labels.join(" + "),
        args.baud,
        args.fps
    );

    // 清屏 + 重置走第一条链路就够：两条链路入同一个队列，板子状态是全局的
    let ports: Vec<String> = links.iter().map(|(port, _)| port.clone()).collect();
    let mut senders: Vec<Link> = links.into_iter().map(|(_, link)| link).collect();
    senders[0].clear().map_err(|error| error.to_string())?;
    senders[0].reset().map_err(|error| error.to_string())?;

    let target = args.fps.clamp(MIN_FPS, MAX_FPS);
    let shared = Arc::new(Shared {
        slot: Mutex::new(VecDeque::new()),
        target: AtomicU32::new((target * TARGET_SCALE) as u32),
        stats: Mutex::new(Stats {
            target,
            ..Stats::default()
        }),
        alive: (0..senders.len()).map(|_| AtomicBool::new(true)).collect(),
        busy: Mutex::new(0.0),
        frame_no: AtomicU32::new(0),
        refresh: AtomicBool::new(false),
        last_adjust: Mutex::new(None),
        started: Instant::now(),
        port_batches: (0..senders.len()).map(|_| AtomicU64::new(0)).collect(),
        port_bytes: (0..senders.len()).map(|_| AtomicU64::new(0)).collect(),
        port_us: (0..senders.len()).map(|_| AtomicU64::new(0)).collect(),
        next_report: Mutex::new(None),
        label: labels.join(" + "),
        port_labels: labels,
    });
    let cancel = Arc::new(AtomicBool::new(false));

    let baud = args.baud;
    for (index, link) in senders.into_iter().enumerate() {
        let shared = Arc::clone(&shared);
        let cancel = Arc::clone(&cancel);
        let port = ports[index].clone();
        thread::spawn(move || send_loop(link, shared, cancel, index, port, baud));
    }

    // 抓屏上限固定给 30 fps，实际节奏由 target 控制
    let config = CaptureStreamConfig {
        name,
        include_cursor: args.cursor,
        fps: Some(MAX_FPS),
        cancel_sig: Arc::clone(&cancel),
        sync_sig: Arc::new(AtomicBool::new(false)),
    };

    let mut frame = Frame::new();
    let verbose = args.verbose;
    let mut last_callback: Option<Instant> = None;
    let result = screen_capture_wayland_wlr::capture_output_stream(config, |mut data| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        // 队列里已经堆了两帧（在发 + 备好）就不再抓：抓屏永远不等串口
        if shared.slot.lock().expect("slot poisoned").len() >= 2 {
            return;
        }

        let started = Instant::now();
        let gap = last_callback.map_or(Duration::ZERO, |last| started.duration_since(last));
        last_callback = Some(started);
        let (stride, height) = (data.data.width as usize, data.data.height as usize);
        if shared.refresh.swap(false, Ordering::Relaxed) {
            // 有口挂掉过：整屏重推，把漏掉的块补上
            frame.invalidate();
        }
        if !frame.update(&mut data.data.pixel_data, stride, height) {
            shared.fail(
                &cancel,
                format!("unexpected frame buffer {stride}x{height}"),
            );
            return;
        }

        let mut tiles = Vec::with_capacity(frame.dirty().len());
        for &index in frame.dirty() {
            tiles.push(frame.tile(index));
        }
        let dirty = tiles.len();

        if let Ok(mut stats) = shared.stats.lock() {
            stats.captured += 1;
            stats.dirty = dirty;
            stats.capture_us = data.capture_time.as_micros() as u64;
            stats.loop_us = gap.as_micros() as u64;
        }
        if verbose {
            println!(
                "captured frame {} dirty {dirty} capture {:?} scale {:?}",
                data.frame_index,
                data.capture_time,
                started.elapsed()
            );
        }
        if !tiles.is_empty() {
            publish(&shared, tiles);
        }
    });

    cancel.store(true, Ordering::Relaxed);
    if let Some(error) = shared.stats.lock().expect("stats poisoned").error.clone() {
        return Err(error);
    }
    result.map_err(|error| error.to_string())?;
    Ok(())
}

/// 自适应粒度：把一帧切成约 `MESSAGES_PER_FRAME` 条消息，块数夹在 `1..=MAX_BATCH`。
/// 小帧（<40 块）一条一块——延迟最低，消息总数本来就不多；大帧多块一条，
/// 摊薄每消息约 15 字节帧头与一次系统调用，否则全屏 300 块要多付三四成链路时间。
fn batch_for(tiles: usize) -> usize {
    tiles.div_ceil(MESSAGES_PER_FRAME).clamp(1, MAX_BATCH)
}

/// 组一帧 Job 发布到槽位；上一帧还没推完就放弃这一帧。
fn publish(shared: &Shared, tiles: Vec<Tile>) {
    let mut slot = shared.slot.lock().expect("slot poisoned");
    // 队里已经有「在发 + 备好」两帧就不再收：抓屏永远不等链路。
    // 腾位只能弹已取完的队首，发到一半的帧不能丢，否则那些块永久停旧。
    while slot.len() >= 2 {
        if slot
            .front()
            .is_some_and(|front| front.cursor.load(Ordering::Relaxed) >= front.batches)
        {
            slot.pop_front();
            continue;
        }
        return;
    }
    let frame_no = (shared.frame_no.fetch_add(1, Ordering::Relaxed) + 1) as u16;
    let batch = batch_for(tiles.len());
    let batches = tiles.len().div_ceil(batch);
    slot.push_back(Arc::new(Job {
        frame_no,
        tiles: Arc::new(tiles),
        cursor: AtomicUsize::new(0),
        batch,
        batches,
        done: AtomicUsize::new(0),
        sent_all: AtomicBool::new(false),
        published: Instant::now(),
    }));
}

/// ping 一次拿板子状态：成功记帧率/忙碌毫秒、失败计数；连着 `PING_STRIKE` 次
/// 不通就判死（一次超时可能是被挤，连着就是链路断了）。
fn probe(link: &mut Link, shared: &Shared, cancel: &AtomicBool, index: usize, misses: &mut u32) {
    match link.ping() {
        Ok(reply) => {
            *misses = 0;
            shared.observe(reply);
        }
        Err(LinkError::Timeout(_)) => {
            *misses += 1;
            if *misses >= PING_STRIKE {
                *misses = 0;
                mark_dead(shared, cancel, index, "ping timeout".into());
            }
        }
        Err(LinkError::Fatal(error)) => mark_dead(shared, cancel, index, error),
    }
}

/// 单个口的发送线程：逐块抢活只管发——数据不等应答，丢了就丢了，画面靠后续帧补。
/// 板子状态只能从控制命令拿：活口里序号最小的口每 500 ms 顺路 ping 一次。
/// 死口先 ping 原连接复活；重开会复位板子，要等其它口全挂才试，且至少隔 REVIVE_REOPEN。
fn send_loop(
    mut link: Link,
    shared: Arc<Shared>,
    cancel: Arc<AtomicBool>,
    index: usize,
    port: String,
    baud: u32,
) {
    let mut last_ping: Option<Instant> = None;
    let mut last_reopen: Option<Instant> = None;
    let mut ping_misses: u32 = 0;
    while !cancel.load(Ordering::Relaxed) {
        if !shared.alive[index].load(Ordering::Relaxed) {
            // 死口复活：先 ping 原连接（不复位板子）；重开只在万不得已时试
            if last_ping.map_or(true, |at| at.elapsed() >= PING) {
                last_ping = Some(Instant::now());
                let others_alive = (0..shared.alive.len())
                    .any(|other| other != index && shared.alive[other].load(Ordering::Relaxed));
                let mut revived = link.ping().is_ok();
                if !revived
                    && !others_alive
                    && last_reopen.map_or(true, |at| at.elapsed() >= REVIVE_REOPEN)
                {
                    last_reopen = Some(Instant::now());
                    if let Ok(mut fresh) = Link::open(&port, baud)
                        && fresh.ping().is_ok()
                    {
                        link = fresh;
                        revived = true;
                    }
                }
                if revived {
                    ping_misses = 0;
                    shared.alive[index].store(true, Ordering::Relaxed);
                    eprintln!("link {index} back online");
                }
            }
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        let job = shared.slot.lock().expect("slot poisoned").front().cloned();
        let Some(job) = job else {
            // 空闲：ping 一来拿板子状态、二来看链路死活
            if last_ping.map_or(true, |at| at.elapsed() >= PING) {
                last_ping = Some(Instant::now());
                probe(&mut link, &shared, &cancel, index, &mut ping_misses);
            }
            thread::sleep(Duration::from_millis(1));
            continue;
        };
        if job.sent_all.load(Ordering::Relaxed) {
            // 这一帧发完了：从队首弹掉，让备好的下一帧顶上
            let mut slot = shared.slot.lock().expect("slot poisoned");
            if slot.front().is_some_and(|front| Arc::ptr_eq(front, &job)) {
                slot.pop_front();
            }
            continue;
        }
        // 数据不再回应答，板子状态只能从控制应答里拿：
        // 活口里序号最小的口每 500 ms 顺路 ping 一次
        if last_ping.map_or(true, |at| at.elapsed() >= PING)
            && (0..shared.alive.len()).find(|&other| shared.alive[other].load(Ordering::Relaxed))
                == Some(index)
        {
            last_ping = Some(Instant::now());
            probe(&mut link, &shared, &cancel, index, &mut ping_misses);
        }
        let batch = job.cursor.fetch_add(1, Ordering::Relaxed);
        if batch >= job.batches {
            // 没活了：整帧收尾由拿到最后一块的线程做，这里等下一帧
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        let start = batch * job.batch;
        let end = (start + job.batch).min(job.tiles.len());
        let is_last = batch + 1 == job.batches;
        let tiles = &job.tiles[start..end];
        let sent_at = Instant::now();
        // batch/total 板子不解析（超 255 会绕回），只带出去方便诊断
        let result = link.send_tiles(
            job.frame_no,
            (batch + 1) as u8,
            job.batches as u8,
            tiles,
            is_last,
        );
        let sent_us = sent_at.elapsed().as_micros() as u64;
        shared.port_us[index].fetch_add(sent_us, Ordering::Relaxed);
        match result {
            Ok(encoded) => {
                shared.port_batches[index].fetch_add(1, Ordering::Relaxed);
                shared.port_bytes[index].fetch_add(encoded as u64, Ordering::Relaxed);
                if let Ok(mut stats) = shared.stats.lock() {
                    stats.tiles += (end - start) as u64;
                    stats.bytes += encoded as u64;
                    stats.raw_bytes += (end - start) as u64 * (TILE * TILE * 2) as u64;
                }
                let done = job.done.fetch_add(1, Ordering::Relaxed) + 1;
                if done == job.batches {
                    finish_frame(&shared, &job);
                }
            }
            // 写失败（含写超时：半截消息已把流搞乱）判死，块还回去让别人接手
            Err(error) => {
                job.cursor.fetch_sub(1, Ordering::Relaxed);
                mark_dead(&shared, &cancel, index, error.to_string());
            }
        }
    }
}

/// 判死并触发整屏补推；所有口都挂了才整体退出。
fn mark_dead(shared: &Shared, cancel: &AtomicBool, index: usize, error: String) {
    shared.alive[index].store(false, Ordering::Relaxed);
    shared.refresh.store(true, Ordering::Relaxed);
    eprintln!("link {index} dead: {error}");
    if shared
        .alive
        .iter()
        .all(|alive| !alive.load(Ordering::Relaxed))
    {
        shared.fail(cancel, format!("all links dead: {error}"));
    }
}

/// 整帧写完的收尾：记账并放行下一帧。数据不等应答，渲染在板子侧异步追赶，
/// 丢的块要么被后续帧覆盖、要么在判死后的整屏重推里补上。
fn finish_frame(shared: &Shared, job: &Job) {
    if let Ok(mut stats) = shared.stats.lock() {
        stats.send_us = job.published.elapsed().as_micros() as u64;
        stats.send_max_us = stats.send_max_us.max(stats.send_us);
        stats.sent += 1;
    }
    adjust(shared);
    job.sent_all.store(true, Ordering::Release);

    // 状态行由收尾线程打印，间隔 1 秒
    let due = {
        let mut next = shared.next_report.lock().expect("report poisoned");
        let due = next.map_or(true, |at| at.elapsed() >= REPORT);
        if due {
            *next = Some(Instant::now());
        }
        due
    };
    if due {
        print_stats(shared);
    }
}

/// 板子打满一帧要 busy 毫秒，连轴转的上限就是 1000/busy；只用到八成，剩下的留给链路空转。
fn adjust(shared: &Shared) {
    let due = {
        let mut last = shared.last_adjust.lock().expect("adjust poisoned");
        let due = last.map_or(true, |at| at.elapsed() >= ADJUST);
        if due {
            *last = Some(Instant::now());
        }
        due
    };
    if !due {
        return;
    }
    let busy = *shared.busy.lock().expect("busy poisoned");
    if busy <= 0.0 {
        return;
    }
    let wanted = (1000.0 / busy).clamp(MIN_FPS, MAX_FPS);
    let current = f64::from(shared.target.load(Ordering::Relaxed)) / TARGET_SCALE;
    // 慢升快降
    let target = if wanted < current {
        wanted
    } else {
        (current + 1.0).min(wanted)
    };
    shared
        .target
        .store((target * TARGET_SCALE) as u32, Ordering::Relaxed);
    if let Ok(mut stats) = shared.stats.lock() {
        stats.target = target;
    }
}

fn print_stats(shared: &Shared) {
    let stats = shared.stats.lock().expect("stats poisoned");
    let seconds = shared.started.elapsed().as_secs_f64().max(0.001);
    let per_port: Vec<String> = shared
        .port_labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let batches = shared.port_batches[index].load(Ordering::Relaxed) as f64 / seconds;
            let kbs = shared.port_bytes[index].load(Ordering::Relaxed) as f64 / 1024.0 / seconds;
            let per_batch = if batches > 0.0 {
                shared.port_us[index].load(Ordering::Relaxed) as f64
                    / 1000.0
                    / shared.port_batches[index].load(Ordering::Relaxed) as f64
            } else {
                0.0
            };
            format!("{label} {batches:.1} t/s {kbs:.0} KB/s {per_batch:.0} ms/t")
        })
        .collect();
    println!(
        "[{}] {} captured ({:.1} fps), {} sent ({:.1} fps), target {:.0}, board {} fps (busy {} ms), {} dirty, {} tiles ({:.0} KB/s, {:.0}% of raw), send {} us (max {}), loop {} us, capture {} us | {}",
        shared.label,
        stats.captured,
        stats.captured as f64 / seconds,
        stats.sent,
        stats.sent as f64 / seconds,
        stats.target,
        stats.board_fps,
        stats.busy_ms,
        stats.dirty,
        stats.tiles,
        stats.bytes as f64 / 1024.0 / seconds,
        100.0 * stats.bytes as f64 / (stats.raw_bytes.max(1)) as f64,
        stats.send_us,
        stats.send_max_us,
        stats.loop_us,
        stats.capture_us,
        per_port.join(", "),
    );
}
