//! BLE：常驻被动扫描 + 跟踪器/监控指纹分类（trouble-host EventHandler）+ 广播名轮播（spammer）。
//! esp-radio 的 coex 特性让 BLE 与 WiFi 共存（wifi-build 形态）；栈启动后常驻（blob 无法干净去初始化）。
//! 菜单 app 只切换日志模式与 spammer 开关，扫描持续运行。

use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU32, Ordering};

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use minio_rs_core::trackers;
use static_cell::StaticCell;
use trouble_host::prelude::*;

use crate::radio;

/// spammer 轮播间隔
const SPAM_ROTATE: Duration = Duration::from_secs(2);
/// 无请求时的轮询间隔
const IDLE_POLL: Duration = Duration::from_millis(200);
/// 出错后的退避
const ERR_BACKOFF: Duration = Duration::from_millis(500);

/// BLE 栈是否已启动
pub static BLE_UP: AtomicBool = AtomicBool::new(false);
/// 已启动的栈缓存（hid 模块要拿它建 GATT 服务）。
/// 栈内含 Cell/RefCell 不满足 Sync，用 AtomicPtr：start 里写一次，之后只读
static STACK_PTR: AtomicPtr<BleStack> = AtomicPtr::new(core::ptr::null_mut());

/// 取已启动的 BLE 栈；未启动返回 None
pub fn stack() -> Option<&'static BleStack> {
    let p = STACK_PTR.load(Ordering::Acquire);
    if p.is_null() {
        None
    } else {
        // 指针只在 start 里写入一次且永不释放
        Some(unsafe { &*p })
    }
}
/// spammer 开关
pub static SPAM_ON: AtomicBool = AtomicBool::new(false);

/// 广播名轮播列表：手机扫描列表里可见的名字洪泛
const SPAM_NAMES: [&[u8]; 5] = [
    b"AirPods Pro",
    b"Galaxy Buds",
    b"Tile Mate",
    b"SmartTag2",
    b"Free Vending",
];

/// 日志模式，菜单 app 切换
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LogMode {
    /// 只打跟踪器命中
    Trackers = 0,
    /// 全量：命中 + 原始 adv 十六进制
    All = 1,
    /// 监控：命中（分离态强调）+ 设备名指纹
    Surveillance = 2,
}

static LOG_MODE: AtomicU8 = AtomicU8::new(LogMode::Trackers as u8);
/// 已收到的广播报告批数：扫描会话靠它感知“报告已到、该重启扫描了”
static SCANS_SEEN: AtomicU32 = AtomicU32::new(0);

/// 切换日志模式
pub fn set_mode(mode: LogMode) {
    LOG_MODE.store(mode as u8, Ordering::Relaxed);
}

fn mode() -> LogMode {
    match LOG_MODE.load(Ordering::Relaxed) {
        1 => LogMode::All,
        2 => LogMode::Surveillance,
        _ => LogMode::Trackers,
    }
}

/// BLE 控制器带 10 个命令缓冲（ble-led 同款）
type BleController = ExternalController<esp_radio::ble::controller::BleConnector<'static>, 10>;
/// trouble-host 栈
pub type BleStack = Stack<'static, BleController, DefaultPacketPool>;

/// 启动 BLE 栈 + 常驻被动扫描 + spammer 任务
pub fn start(spawner: &Spawner, bt: esp_hal::peripherals::BT<'static>) -> Result<(), &'static str> {
    if BLE_UP.load(Ordering::Relaxed) {
        return Ok(());
    }
    // 射频租约：BLE 独占（与 WiFi 的空口共存由 coex blob 处理）
    if let Err(e) = radio::acquire(minio_rs_core::lease::RadioKind::Ble) {
        return Err(match e {
            minio_rs_core::lease::LeaseError::Busy(_) => "radio busy",
            _ => "unsupported",
        });
    }

    let connector = esp_radio::ble::controller::BleConnector::new(
        bt,
        esp_radio::ble::Config::default(),
    )
    .map_err(|_| "BLE controller init failed")?;
    let controller = ExternalController::new(connector);

    static RESOURCES: StaticCell<HostResources<DefaultPacketPool, 1, 3>> = StaticCell::new();
    let stack: &'static BleStack = {
        let s = trouble_host::new(controller, RESOURCES.init(HostResources::new())).build();
        static S: StaticCell<BleStack> = StaticCell::new();
        S.init(s)
    };

    spawner.spawn(runner_task(stack.runner()).expect("spawn ble runner"));
    spawner.spawn(scanner_task(stack).expect("spawn scanner"));
    spawner
        .spawn(spammer_task(stack.peripheral()).expect("spawn spammer"));
    STACK_PTR.store(stack as *const _ as *mut _, Ordering::Release);

    BLE_UP.store(true, Ordering::Relaxed);
    info!("ble up, passive scan running");
    Ok(())
}

/// 扫描报告处理：分类打日志
struct ScanHandler;

impl EventHandler for ScanHandler {
    fn on_adv_reports(&self, reports: LeAdvReportsIter) {
        SCANS_SEEN.fetch_add(1, Ordering::Relaxed);
        for report in reports.flatten() {
            log_report(report.data, report.rssi);
        }
    }
}

/// 栈驱动：扫描报告经 EventHandler 进 ScanHandler
#[embassy_executor::task]
async fn runner_task(mut runner: Runner<'static, BleController, DefaultPacketPool>) {
    if let Err(e) = runner.run_with_handler(&ScanHandler).await {
        warn!("ble stack exited: {}", defmt::Debug2Format(&e));
    }
}

/// 常驻被动扫描：一轮报告到达即重启扫描
#[embassy_executor::task]
async fn scanner_task(stack: &'static BleStack) {
    let mut central = stack.central();
    let mut scanner = Scanner::new(&mut central);
    let config = ScanConfig {
        // 被动扫描：不发 SCAN_REQ，纯收广播
        active: false,
        ..ScanConfig::default()
    };
    loop {
        match scanner.scan(&config).await {
            Ok(_session) => {
                // 传统扫描收到一批报告即自停；等计数变化后走 Drop 停扫描再重启。
                // 会话必须在 match 臂作用域内存活，否则 Drop 立刻把扫描停了
                let before = SCANS_SEEN.load(Ordering::Relaxed);
                while SCANS_SEEN.load(Ordering::Relaxed) == before {
                    Timer::after(Duration::from_millis(50)).await;
                }
            }
            Err(e) => {
                warn!("scan error: {}", defmt::Debug2Format(&e));
                Timer::after(ERR_BACKOFF).await;
            }
        }
    }
}

/// 广播名轮播（spammer）：SPAM_ON 时每 SPAM_ROTATE 换一个名字广播；
/// 不 accept 连接，到点丢弃 Advertiser（Drop 停广播）换下一个名字
#[embassy_executor::task]
async fn spammer_task(mut peripheral: Peripheral<'static, BleController, DefaultPacketPool>) {
    let mut i = 0usize;
    loop {
        if !SPAM_ON.load(Ordering::Relaxed) {
            Timer::after(IDLE_POLL).await;
            continue;
        }
        let structures = [
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(SPAM_NAMES[i]),
        ];
        let mut adv = [0u8; 31];
        match AdStructure::encode_slice(&structures, &mut adv) {
            Ok(n) => {
                match peripheral
                    .advertise(
                        &Default::default(),
                        Advertisement::ConnectableScannableUndirected {
                            adv_data: &adv[..n],
                            scan_data: &[],
                        },
                    )
                    .await
                {
                    Ok(_advertiser) => {
                        info!(
                            "spam advertising: {}",
                            core::str::from_utf8(SPAM_NAMES[i]).unwrap_or("?")
                        );
                        // 轮播间隔；离开作用域即 Drop 停广播
                        Timer::after(SPAM_ROTATE).await;
                    }
                    Err(e) => {
                        warn!("spam advertise failed: {}", defmt::Debug2Format(&e));
                        Timer::after(ERR_BACKOFF).await;
                    }
                }
            }
            Err(_) => Timer::after(ERR_BACKOFF).await,
        }
        i = (i + 1) % SPAM_NAMES.len();
    }
}

/// 单条广播报告 → 按模式分类打日志
fn log_report(data: &[u8], rssi: i8) {
    if let Some(hit) = trackers::classify_adv(data) {
        if hit.separated.unwrap_or(false) {
            warn!(
                "tracker {} SEPARATED rssi {}",
                hit.kind.name(),
                rssi
            );
        } else {
            info!("tracker {} rssi {}", hit.kind.name(), rssi);
        }
        return;
    }
    match mode() {
        LogMode::Trackers => {}
        LogMode::All => info!("adv rssi {} {=[u8]:02x}", rssi, data),
        LogMode::Surveillance => {
            if let Some(tag) = trackers::surveillance_name(data) {
                warn!("surveillance {} rssi {}", tag, rssi);
            }
        }
    }
}
