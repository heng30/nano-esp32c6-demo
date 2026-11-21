//! 菜单叶子 app 的生命周期分发，对应上游 `app_manager`。
//! 真实功能由后续任务逐个接入；受阻功能在这里显式声明原因，不假装修好了。

use defmt::{info, warn};
use minio_rs_core::config::SCREENSAVER_STEPS;
use minio_rs_core::menu::{self, MenuId};

use crate::settings;
#[cfg(feature = "wifi-build")]
use crate::ble;
#[cfg(feature = "wifi-build")]
use crate::hid;
#[cfg(feature = "wifi-build")]
use crate::net;
#[cfg(feature = "ieee154-build")]
use crate::radio154;

/// 跨形态不可用项：另一固件形态才有的功能
fn build_reason(id: MenuId) -> Option<&'static str> {
    #[cfg(feature = "wifi-build")]
    if matches!(
        id,
        MenuId::Sniffer154 | MenuId::Spoof154 | MenuId::ThreadBroadcast | MenuId::Sniffer6LoWpan
    ) {
        return Some("not in wifi build, flash the ieee154-build firmware");
    }
    #[cfg(feature = "ieee154-build")]
    if matches!(
        id,
        MenuId::WifiCaptivePortal
            | MenuId::WifiWardriving
            | MenuId::BleTrackers
            | MenuId::BleAdvSniffer
            | MenuId::BleSpammer
            | MenuId::Surveillance
    ) {
        return Some("not in 154 build, flash the wifi-build firmware");
    }
    None
}

/// ⬛ 受阻功能的原因，对照 docs/PORTING.md 的 stub 清单
fn stub_reason(id: MenuId) -> Option<&'static str> {
    match id {
        MenuId::WifiSniffer
        | MenuId::WifiDeauther
        | MenuId::WifiCatDos
        | MenuId::WifiSsidSpam
        | MenuId::WifiDroneId
        | MenuId::Spectrum
        | MenuId::Sdr => Some("esp-radio blob exposes no promiscuous/raw 802.11 API"),
        MenuId::ZigbeeLight | MenuId::ZigbeeSwitch => {
            Some("no Zigbee network stack exists in Rust")
        }
        _ => None,
    }
}

fn onoff(v: bool) -> &'static str {
    if v { "on" } else { "off" }
}

/// 屏保超时下一档（循环）；未知名次回到第一档
fn next_screensaver(cur: u16) -> u16 {
    let steps = &SCREENSAVER_STEPS;
    match steps.iter().position(|&s| s == cur) {
        Some(i) => steps[(i + 1) % steps.len()],
        None => steps[0],
    }
}

fn log_screensaver(secs: u16) {
    if secs == 0 {
        info!("screensaver: off");
    } else {
        info!("screensaver: {} s", secs);
    }
}

/// on_enter：跨形态/stub 打原因；设置类显示当前值；其余待实现
pub fn on_enter(id: MenuId) {
    if let Some(reason) = build_reason(id) {
        warn!("unavailable: {} ({})", menu::name(id), reason);
        return;
    }
    if let Some(reason) = stub_reason(id) {
        warn!("unavailable: {} ({})", menu::name(id), reason);
        return;
    }
    match id {
        MenuId::SetStealth => info!("stealth: {}", onoff(settings::current().stealth)),
        MenuId::SetMenuStyle => info!("menu style: {}", settings::current().menu_style.name()),
        MenuId::SetDisplay => log_screensaver(settings::current().screensaver_secs),
        MenuId::SetLogs => info!("log level pinned by DEFMT_LOG env in .cargo/config.toml"),
        MenuId::SetSdCard => {
            info!("sd status in log, short press writes self-test file");
        }
        MenuId::SetWifi => {
            let c = settings::current();
            if c.ssid_str().is_empty() {
                info!("STA credentials unset, provisioning lands with wifi task");
            } else {
                info!("STA ssid: {}", c.ssid_str());
            }
        }
        MenuId::SetLightSleep => info!("short press: light sleep 10 s (timer wake)"),
        MenuId::AboutVersion => crate::ota::version_report(),
        MenuId::AboutCredits => crate::ota::credits_report(),
        MenuId::AboutOta => crate::ota::ota_report(),
        MenuId::Gps => info!(
            "gps: NMEA parser ready, uart not wired (see README: gps tx -> gpio17)"
        ),
        MenuId::FileManager => info!(
            "file manager: web /files ready, short press lists sd root"
        ),
        #[cfg(feature = "wifi-build")]
        MenuId::WifiCaptivePortal => {
            info!("portal: AP `{}` pass `{}`", net::SSID, net::PASSWORD)
        }
        #[cfg(feature = "wifi-build")]
        MenuId::WifiWardriving => {
            info!("wardriving scan requested, results in log");
            net::request_scan();
        }
        #[cfg(feature = "wifi-build")]
        MenuId::BleTrackers => {
            ble::set_mode(ble::LogMode::Trackers);
            info!("ble trackers scan: classified hits only");
        }
        #[cfg(feature = "wifi-build")]
        MenuId::BleAdvSniffer => {
            ble::set_mode(ble::LogMode::All);
            info!("ble adv sniffer: raw advertisements + classified");
        }
        #[cfg(feature = "wifi-build")]
        MenuId::Surveillance => {
            ble::set_mode(ble::LogMode::Surveillance);
            info!("surveillance scan: tracker hits + name fingerprints");
        }
        #[cfg(feature = "wifi-build")]
        MenuId::BleSpammer => {
            ble::SPAM_ON.store(true, core::sync::atomic::Ordering::Relaxed);
            info!("ble spammer: rotating advertisement names");
        }
        #[cfg(feature = "wifi-build")]
        MenuId::BleHid => {
            // 广播洪泛与 HID 广播互斥，进 HID 先停 spammer
            ble::SPAM_ON.store(false, core::sync::atomic::Ordering::Relaxed);
            info!("hid keyboard: pair from phone, short press types minio-rs");
        }
        #[cfg(feature = "ieee154-build")]
        MenuId::Sniffer154 | MenuId::Sniffer6LoWpan => {
            info!("802.15.4 sniffer, short press: next channel");
        }
        #[cfg(feature = "ieee154-build")]
        MenuId::Spoof154 => {
            info!("spoof: short press sends a beacon request");
        }
        #[cfg(feature = "ieee154-build")]
        MenuId::ThreadBroadcast => {
            info!("thread broadcast: short press sends a broadcast data frame");
        }
        _ => info!("entered {} (implementation pending)", menu::name(id)),
    }
}

/// on_input：短按改值并立即落盘
pub fn on_input(id: MenuId) {
    match id {
        #[cfg(feature = "wifi-build")]
        MenuId::WifiWardriving => {
            // 短按重扫一轮
            info!("wardriving rescan");
            net::request_scan();
        }
        #[cfg(feature = "wifi-build")]
        MenuId::BleHid => {
            info!("hid: type request");
            hid::request_type();
        }
        #[cfg(feature = "ieee154-build")]
        MenuId::Sniffer154 | MenuId::Sniffer6LoWpan => {
            if radio154::SNIFFER_UP.load(core::sync::atomic::Ordering::Relaxed) {
                radio154::request_next_channel();
            } else {
                info!("sniffer not running, re-enter to start");
            }
        }
        #[cfg(feature = "ieee154-build")]
        MenuId::Spoof154 => {
            if radio154::SNIFFER_UP.load(core::sync::atomic::Ordering::Relaxed) {
                radio154::request_tx(radio154::TxKind::BeaconRequest);
                info!("spoof: beacon request queued");
            } else {
                info!("sniffer not running, re-enter to start");
            }
        }
        #[cfg(feature = "ieee154-build")]
        MenuId::ThreadBroadcast => {
            if radio154::SNIFFER_UP.load(core::sync::atomic::Ordering::Relaxed) {
                radio154::request_tx(radio154::TxKind::BroadcastData);
                info!("broadcast: data frame queued");
            } else {
                info!("sniffer not running, re-enter to start");
            }
        }
        MenuId::FileManager => {
            let r = crate::sd::with_sd(|s| {
                let mut body = alloc::string::String::new();
                let res = s.list_root(|name, size| {
                    use core::fmt::Write as _;
                    let _ = write!(body, "{} {} bytes\n", name, size);
                });
                (res, body)
            });
            match r {
                Some((Ok(()), body)) => info!("sd root:\n{}", body.as_str()),
                Some((Err(e), _)) => warn!("list failed: {}", e),
                None => info!("sd absent"),
            }
        }
        MenuId::SetStealth => {
            settings::update(|c| c.stealth = !c.stealth);
            info!("stealth: {}", onoff(settings::current().stealth));
        }
        MenuId::SetMenuStyle => {
            settings::update(|c| c.menu_style = c.menu_style.cycle());
            info!("menu style: {}", settings::current().menu_style.name());
        }
        MenuId::SetDisplay => {
            settings::update(|c| c.screensaver_secs = next_screensaver(c.screensaver_secs));
            log_screensaver(settings::current().screensaver_secs);
        }
        _ => {}
    }
}

/// on_exit
pub fn on_exit(id: MenuId) {
    info!("exit {}", menu::name(id));
}
