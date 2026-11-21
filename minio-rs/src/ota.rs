//! About 版本信息与 OTA 槽位查询。
//! OTA 双分区要求分区表含 `otadata` + 至少两个 app 槽（ota_0/ota_1）。
//! esp-hal 默认分区表是单个 factory，因此本模块如实报告槽位状态，
//! 缺槽时直接说清楚要换什么分区表，不假装能升级。

use defmt::info;
use esp_bootloader_esp_idf::ota_updater::OtaUpdater;
use esp_bootloader_esp_idf::partitions::AppPartitionSubType;

use crate::settings;

/// 分区表读入缓冲长度（ESP32-C6 固定 0xC00）
const PARTITION_TABLE_SIZE: usize = 0xC00;

/// 固件版本（Cargo 包版本）
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 打印 About 版本页
pub fn version_report() {
    info!("minio-rs v{}", VERSION);
    info!("target: esp32c6 riscv32imac, esp-hal 1.2 + esp-rtos + embassy");
    info!("license: MIT OR Apache-2.0");
}

/// 打印致谢页
pub fn credits_report() {
    info!("upstream: ElectronicCats Minino (GPL-3.0)");
    info!("port: minio-rs by nano-esp32c6-demos");
}

/// OTA 槽位状态：成功返回当前选中槽，失败返回原因
pub fn ota_status() -> Result<AppPartitionSubType, &'static str> {
    let mut buf = [0u8; PARTITION_TABLE_SIZE];
    settings::with_flash(|flash| {
        let mut updater = OtaUpdater::new(flash, &mut buf).map_err(|_| {
            "partition table has no ota slots: flash a table with otadata + ota_0/ota_1"
        })?;
        updater
            .selected_partition()
            .map_err(|_| "no ota partition selected")
    })
    .unwrap_or(Err("flash not initialized"))
}

/// 打印 OTA 状态页
pub fn ota_report() {
    match ota_status() {
        Ok(slot) => info!("ota slot: {:?} (update path available)", slot),
        Err(e) => info!("ota unavailable: {}", e),
    }
}
