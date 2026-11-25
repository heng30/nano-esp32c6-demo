//! 副屏上位机（命令行）：抓屏线程缩放 + 分块 diff 后投递到**单帧槽位**，
//! 发送线程取走就立刻走串口。抓屏不等串口，串口也不等抓屏。

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use clap::Parser;
use screen_capture::CaptureStreamConfig;

mod frame;
mod link;

use frame::{Frame, HEIGHT, TILE, WIDTH};
use link::{Link, Report};

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
/// 一批最多几个 tile，与固件一致。
const BATCH: usize = 64;

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
    /// 依次试，第一条能应答的链路被采用
    #[arg(long, value_delimiter = ',')]
    serial: Vec<String>,

    /// 波特率（USB CDC 链路忽略；走 UART0 时要与固件一致）
    #[arg(long, default_value_t = DEFAULT_BAUD)]
    baud: u32,

    /// 初始目标帧率，运行中按板子上报的 busy_ms 自动调整（上限 30）
    #[arg(long, default_value_t = DEFAULT_FPS)]
    fps: f64,
}

/// 默认波特率（USB CDC 链路会忽略；走 UART0 时要与固件一致）。
const DEFAULT_BAUD: u32 = 2_000_000;

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

/// 依次试每个候选串口，返回第一个能应答 `CMD_PING` 的连接。
fn connect(candidates: &[String], baud: u32) -> Result<(String, Link), String> {
    let mut tried = Vec::new();
    for port in candidates {
        match Link::open(port, baud).and_then(|mut link| link.ping().map(|_| link)) {
            Ok(link) => return Ok((port.clone(), link)),
            Err(error) => tried.push(format!("  {port}: {error}")),
        }
    }
    Err(format!("no board answered, tried:\n{}", tried.join("\n")))
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

/// 抓屏线程与发送线程共享的状态。
struct Shared {
    /// 最新一帧的脏块。发送线程没取走之前新帧直接覆盖它——只缓冲一帧。
    slot: Mutex<Option<Vec<frame::Tile>>>,
    /// 目标帧率 × 10。
    target: AtomicU32,
    stats: Mutex<Stats>,
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
    /// 上一帧从开始发送到发完的耗时（微秒）。
    send_us: u64,
    /// 发送一帧的最长耗时（微秒），看链路峰值。
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

    fn observe(&self, report: Report) -> f64 {
        let mut stats = self.stats.lock().expect("stats poisoned");
        stats.board_fps = report.fps;
        stats.busy_ms = report.busy_ms;
        stats.target
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

    let candidates = if args.serial.is_empty() {
        usb_ports()
    } else {
        args.serial.clone()
    };
    if candidates.is_empty() {
        return Err("no usb serial port found".into());
    }

    // 板子两条链路（USB CDC 与 UART0）同时开着，插哪个口都行：谁先应答就用谁
    let (port, mut link) = connect(&candidates, args.baud)?;

    println!(
        "output {name} -> {port} @ {} baud, scale to {WIDTH}x{HEIGHT}, {} fps",
        args.baud, args.fps
    );

    // 控制命令：先把屏清成黑的，免得上一轮残留的像素留到第一帧
    link.clear()?;

    let target = args.fps.clamp(MIN_FPS, MAX_FPS);
    let shared = Arc::new(Shared {
        slot: Mutex::new(None),
        target: AtomicU32::new((target * TARGET_SCALE) as u32),
        stats: Mutex::new(Stats {
            target,
            ..Stats::default()
        }),
    });
    let cancel = Arc::new(AtomicBool::new(false));

    let sender = {
        let shared = Arc::clone(&shared);
        let cancel = Arc::clone(&cancel);
        thread::spawn(move || send_loop(link, shared, cancel))
    };

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
        // 上一帧还没发出去就放弃这一帧：抓屏永远不等串口
        if shared.slot.lock().expect("slot poisoned").is_some() {
            return;
        }

        let started = Instant::now();
        let gap = last_callback.map_or(Duration::ZERO, |last| started.duration_since(last));
        last_callback = Some(started);
        let (stride, height) = (data.data.width as usize, data.data.height as usize);
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
            *shared.slot.lock().expect("slot poisoned") = Some(tiles);
        }

        // 按目标帧率控制节奏：这一拍已经花掉的（处理 + 抓屏）从周期里扣掉，余数睡掉。
        // 扣不出余数说明跟不上，直接不睡、也不补帧，所以延迟不会累积。
        let target = f64::from(shared.target.load(Ordering::Relaxed)) / TARGET_SCALE;
        let spent = started.elapsed() + data.capture_time;
        if let Some(rest) = Duration::from_secs_f64(1.0 / target.max(MIN_FPS)).checked_sub(spent) {
            thread::sleep(rest);
        }
    });

    cancel.store(true, Ordering::Relaxed);
    let _ = sender.join();

    if let Some(error) = shared.stats.lock().expect("stats poisoned").error.clone() {
        return Err(error);
    }
    result.map_err(|error| error.to_string())?;
    Ok(())
}

/// 发送线程：槽位里有帧就整帧发完，没有就隔一会儿用控制命令问一次板子状态。
fn send_loop(mut link: Link, shared: Arc<Shared>, cancel: Arc<AtomicBool>) {
    let started = Instant::now();
    let mut report = Instant::now();
    let mut last_ping = Instant::now();
    let mut last_adjust = Instant::now();
    let mut busy_avg = 0.0f64;

    while !cancel.load(Ordering::Relaxed) {
        let job = { shared.slot.lock().expect("slot poisoned").take() };

        let Some(tiles) = job else {
            if last_ping.elapsed() >= PING {
                match link.ping() {
                    Ok(reply) => busy_avg = observe(&shared, reply, busy_avg),
                    Err(error) => return shared.fail(&cancel, error),
                }
                last_ping = Instant::now();
            }
            thread::sleep(Duration::from_millis(1));
            continue;
        };

        let send_start = Instant::now();
        let mut encoded_bytes = 0u64;
        let chunks = tiles.len().div_ceil(BATCH);
        for (index, chunk) in tiles.chunks(BATCH).enumerate() {
            match link.send_tiles(chunk, index + 1 == chunks) {
                Ok((reply, encoded)) => {
                    busy_avg = observe(&shared, reply, busy_avg);
                    encoded_bytes += encoded as u64;
                }
                Err(error) => return shared.fail(&cancel, error),
            }
        }

        if let Ok(mut stats) = shared.stats.lock() {
            stats.send_us = send_start.elapsed().as_micros() as u64;
            stats.send_max_us = stats.send_max_us.max(stats.send_us);
            stats.sent += 1;
            stats.tiles += tiles.len() as u64;
            stats.bytes += encoded_bytes;
            stats.raw_bytes += tiles.len() as u64 * (TILE * TILE * 2) as u64;
        }

        if last_adjust.elapsed() >= ADJUST {
            adjust(&shared, busy_avg);
            last_adjust = Instant::now();
        }

        if report.elapsed() >= REPORT {
            print_stats(&shared, started.elapsed());
            report = Instant::now();
        }
    }
}

/// 记下板子的上报值，并把忙碌时长做指数平滑。
fn observe(shared: &Shared, report: Report, busy_avg: f64) -> f64 {
    shared.observe(report);
    match (busy_avg, report.busy_ms) {
        (_, 0) => busy_avg,
        (0.0, sample) => f64::from(sample),
        (avg, sample) => avg * 0.7 + f64::from(sample) * 0.3,
    }
}

/// 板子打满一帧要 busy 毫秒，连轴转的上限就是 1000/busy；只用到八成，剩下的留给链路空转。
fn adjust(shared: &Shared, busy_avg: f64) {
    if busy_avg <= 0.0 {
        return;
    }
    let wanted = (1000.0 / busy_avg * 0.8).clamp(MIN_FPS, MAX_FPS);
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

fn print_stats(shared: &Shared, elapsed: Duration) {
    let stats = shared.stats.lock().expect("stats poisoned");
    let seconds = elapsed.as_secs_f64().max(0.001);
    println!(
        "{} captured ({:.1} fps), {} sent ({:.1} fps), target {:.0}, board {} fps (busy {} ms), {} dirty, {} tiles ({:.0} KB/s, {:.0}% of raw), send {} us (max {}), loop {} us, capture {} us",
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
    );
}
