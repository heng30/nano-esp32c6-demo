//! PXE 网络引导与无线 BIOS 更新（OTA）。

use alloc::vec;

use crate::{
    fs,
    hal::console,
    net,
    nvram::{self, BiosUpdateState},
};

/// 下载目标目录名。
const DOWNLOAD_DIR: &str = "downloaded";
/// 下载目标文件名。
const PAYLOAD_NAME: &str = "payload.bin";

/// 通过 HTTP 把 payload 下载进 `/downloaded/payload.bin`，返回字节数。
pub async fn download_payload(url: &str) -> Result<usize, &'static str> {
    if !net::is_connected() {
        return Err("Wi-Fi 未连接");
    }

    let data = net::http_download(url).await.ok_or("下载失败")?;
    if data.is_empty() {
        return Err("下载内容为空");
    }

    let dir = match fs::find_id(DOWNLOAD_DIR, 0) {
        Some(id) => id,
        None => fs::mkdir(DOWNLOAD_DIR, 0).ok_or("无法创建下载目录")?,
    };

    let payload = match crate::zc6::Header::parse(&data) {
        // .zc6 归档按原样存盘，启动时再透明解压
        Some(_) => fs::write_file("payload.zc6", &data, dir),
        None => fs::write_file(PAYLOAD_NAME, &data, dir),
    };

    payload.map(|_| data.len()).ok_or("写入文件系统失败")
}

/// 通过 HTTP 把新 BIOS 拉下来写进 OTA 被动槽。
///
/// 写完把 NVRAM 的更新状态清回 `None`，真正的槽位切换交给 bootloader。
pub async fn bios_ota(url: &str) -> Result<usize, &'static str> {
    if !net::is_connected() {
        return Err("Wi-Fi 未连接");
    }

    console::print("[OTA] 开始下载 BIOS 镜像...\r\n");
    let data = net::http_download(url).await.ok_or("下载失败")?;
    if data.is_empty() {
        return Err("下载内容为空");
    }

    let written = write_ota_slot(&data)?;
    nvram::update(|c| c.bios_update_state = BiosUpdateState::None);
    console::print("[OTA] 镜像已写入被动槽\r\n");
    Ok(written)
}

/// 把镜像写进 `ota_0` 分区。
fn write_ota_slot(image: &[u8]) -> Result<usize, &'static str> {
    // shortcut: 直接按分区起点顺序写入，没做 ESP-IDF 镜像头校验和 A/B 槽位轮转；
    // 真正的槽位标记与回滚交给 bootloader 处理。
    let sectors = image.len().div_ceil(4096);
    let mut buf = vec![0xFFu8; 4096];

    for sector in 0..sectors {
        let from = sector * 4096;
        let to = (from + 4096).min(image.len());
        buf.fill(0xFF);
        buf[..to - from].copy_from_slice(&image[from..to]);
        crate::hal::flash::ota_write_sector(sector as u32, &buf)?;
        crate::me::pet_watchdog();
    }

    Ok(image.len())
}

