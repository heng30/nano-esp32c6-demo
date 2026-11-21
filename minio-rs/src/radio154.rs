//! 802.15.4 嗅探/欺骗：esp-radio 的 `Ieee802154` 驱动，promiscuous 轮询。
//! esp-radio 在编译期就禁止 802.15.4 与 WiFi 同二进制（build.rs 强制），
//! 因此本模块只在 `ieee154-build` 形态下编译，WiFi 菜单在该形态下不可用。

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use defmt::{info, warn};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Timer};
use esp_radio::ieee802154::{Config, Ieee802154, RawReceived};
use minio_rs_core::frame154::{self, Addr154};

/// 2.4GHz 802.15.4 信道下界
pub const CHANNEL_MIN: u8 = 11;
/// 2.4GHz 802.15.4 信道上界
pub const CHANNEL_MAX: u8 = 26;
/// 起始信道
const CHANNEL_START: u8 = 15;

/// 嗅探器是否已启动（驱动无法干净去初始化，启动后常驻）
pub static SNIFFER_UP: AtomicBool = AtomicBool::new(false);

/// 信道切换请求
static CH_REQUEST: Signal<CriticalSectionRawMutex, u8> = Signal::new();
/// 发送请求
static TX_REQUEST: Signal<CriticalSectionRawMutex, TxKind> = Signal::new();
/// 当前信道
static CHANNEL: AtomicU8 = AtomicU8::new(CHANNEL_START);

/// 发送帧类别
#[derive(Debug, Clone, Copy)]
pub enum TxKind {
    /// Zigbee 扫描用 Beacon Request 广播
    BeaconRequest,
    /// Thread 风格数据广播
    BroadcastData,
}

/// Beacon Request PSDU：命令帧 + 目的短地址广播；PHY 长度与 FCS 由驱动补
const BEACON_REQUEST: [u8; 8] = [
    0x03,
    0x04,
    0x00,
    0xFF,
    0xFF,
    0xFF,
    0xFF,
    frame154::CMD_BEACON_REQUEST,
];
/// 广播数据帧 PSDU：data + 目的短地址广播
const BROADCAST_DATA: [u8; 7] = [0x01, 0x04, 0x00, 0xFF, 0xFF, 0xFF, 0xFF];

/// 请求切到下一信道（11–26 循环）
pub fn request_next_channel() {
    let mut ch = CHANNEL.load(Ordering::Relaxed) + 1;
    if ch > CHANNEL_MAX {
        ch = CHANNEL_MIN;
    }
    CHANNEL.store(ch, Ordering::Relaxed);
    CH_REQUEST.signal(ch);
}

/// 请求发送一帧
pub fn request_tx(kind: TxKind) {
    TX_REQUEST.signal(kind);
}

/// promiscuous 接收配置
fn sniff_config(channel: u8) -> Config {
    Config {
        promiscuous: true,
        auto_ack_tx: false,
        auto_ack_rx: false,
        enhance_ack_tx: false,
        channel,
        rx_when_idle: true,
        ..Config::default()
    }
}

/// 嗅探任务：轮询收帧打日志，响应信道切换与发送请求
#[embassy_executor::task]
pub async fn sniffer(mut radio: Ieee802154<'static>) {
    let ch = CHANNEL.load(Ordering::Relaxed);
    radio.set_config(sniff_config(ch));
    radio.start_receive();
    SNIFFER_UP.store(true, Ordering::Relaxed);
    info!("802.15.4 sniffer on ch {}", ch);

    loop {
        if let Some(ch) = CH_REQUEST.try_take() {
            radio.set_config(sniff_config(ch));
            radio.start_receive();
            info!("154 channel -> {}", ch);
        }
        if let Some(kind) = TX_REQUEST.try_take() {
            let frame: &[u8] = match kind {
                TxKind::BeaconRequest => &BEACON_REQUEST,
                TxKind::BroadcastData => &BROADCAST_DATA,
            };
            match radio.transmit_raw(frame, true) {
                Ok(()) => info!("154 tx sent: {} bytes", frame.len()),
                Err(e) => warn!("154 tx failed: {}", defmt::Debug2Format(&e)),
            }
        }
        while let Some(RawReceived { data, channel }) = radio.raw_received() {
            log_frame(&data, channel);
        }
        Timer::after(Duration::from_millis(10)).await;
    }
}

/// 解析并打一行帧日志；`data` 是驱动的 RX 缓冲，长度字节在 `[0]`、PSDU 从 `[1]` 起
fn log_frame(data: &[u8], channel: u8) {
    let len = data[0] as usize;
    if len < 3 || 1 + len > data.len() {
        return;
    }
    match frame154::parse(&data[1..1 + len]) {
        Some(f) => {
            let tag = if frame154::is_beacon_request(&f) {
                " beacon-req"
            } else {
                ""
            };
            match (f.src, f.dst) {
                (Addr154::Short(s), Addr154::Short(d)) => info!(
                    "154 ch{} {} seq {} src {=u16:#x} dst {=u16:#x} len{}{}",
                    channel,
                    f.ftype.name(),
                    f.seq,
                    s,
                    d,
                    len,
                    tag
                ),
                _ => info!(
                    "154 ch{} {} seq {} len{}{}",
                    channel,
                    f.ftype.name(),
                    f.seq,
                    len,
                    tag
                ),
            }
        }
        None => info!("154 ch{} unparsed len {}", channel, len),
    }
}
