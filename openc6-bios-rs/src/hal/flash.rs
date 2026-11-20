//! SPI Flash 访问：分区表查找 + 分区内读/写/擦。

use core::cell::RefCell;

use critical_section::Mutex;
use esp_storage::FlashStorage;

/// 分区表在 flash 里的偏移。
/// 分区表最大长度，C6 上固定 0xC00。
const PARTITION_TABLE_SIZE: usize = 0xC00;

/// flash 分区边界。
#[derive(Debug, Clone, Copy, defmt::Format)]
pub struct Partition {
    /// 分区在整片 flash 里的字节偏移。
    pub offset: u32,
    /// 分区字节长度。
    pub size: u32,
}

static FLASH: Mutex<RefCell<Option<FlashStorage<'static>>>> = Mutex::new(RefCell::new(None));

/// `openc6_fs` 分区，`init` 之后才有值。
static FS_PART: Mutex<RefCell<Option<Partition>>> = Mutex::new(RefCell::new(None));
/// `nvs` 分区，`init` 之后才有值。
static NVS_PART: Mutex<RefCell<Option<Partition>>> = Mutex::new(RefCell::new(None));
/// `ota_0` 分区，`init` 之后才有值。
static OTA_PART: Mutex<RefCell<Option<Partition>>> = Mutex::new(RefCell::new(None));

/// 芯片 flash 总容量（字节），`init` 后填充。
static CHIP_SIZE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// 初始化 flash 驱动并解析分区表。
pub fn init(flash_peripheral: esp_hal::peripherals::FLASH<'static>) {
    let flash = FlashStorage::new(flash_peripheral);
    critical_section::with(|cs| FLASH.borrow_ref_mut(cs).replace(flash));

    let mut buf = [0u8; PARTITION_TABLE_SIZE];
    let table = critical_section::with(|cs| {
        let mut guard = FLASH.borrow_ref_mut(cs);
        let flash = guard.as_mut().expect("flash 未初始化");
        esp_bootloader_esp_idf::partitions::read_partition_table(flash, &mut buf)
    });

    let Ok(table) = table else {
        defmt::error!("分区表读取失败");
        return;
    };

    let mut fs_part = None;
    let mut nvs_part = None;
    let mut ota_part = None;
    for entry in table.iter() {
        let part = Partition {
            offset: entry.offset(),
            size: entry.len(),
        };
        match entry.label_as_str() {
            "openc6_fs" => fs_part = Some(part),
            "nvs" => nvs_part = Some(part),
            "ota_0" => ota_part = Some(part),
            _ => {}
        }
    }

    CHIP_SIZE.store(
        critical_section::with(|cs| {
            let guard = FLASH.borrow_ref(cs);
            guard.as_ref().map_or(0, |f| f.capacity() as u32)
        }),
        core::sync::atomic::Ordering::Relaxed,
    );

    critical_section::with(|cs| {
        *FS_PART.borrow_ref_mut(cs) = fs_part;
        *NVS_PART.borrow_ref_mut(cs) = nvs_part;
        *OTA_PART.borrow_ref_mut(cs) = ota_part;
    });

    match fs_part {
        Some(p) => defmt::info!("分区 openc6_fs: {:#x} + {} KB", p.offset, p.size / 1024),
        None => defmt::error!("找不到 openc6_fs 分区"),
    }
}

/// 取某个分区描述。
fn partition(slot: &Mutex<RefCell<Option<Partition>>>) -> Option<Partition> {
    critical_section::with(|cs| *slot.borrow_ref(cs))
}

/// `openc6_fs` 分区描述。
pub fn fs_partition() -> Option<Partition> {
    partition(&FS_PART)
}

/// `ota_0` 分区描述。
pub fn ota_partition() -> Option<Partition> {
    partition(&OTA_PART)
}

/// 擦掉并写入 `ota_0` 分区里的一个扇区。
pub fn ota_write_sector(sector: u32, buf: &[u8]) -> Result<(), &'static str> {
    let Some(p) = ota_partition() else {
        return Err("找不到 ota_0 分区");
    };
    let from = p.offset + sector * 4096;
    if buf.len() > 4096 || from + 4096 > p.offset + p.size {
        return Err("OTA 写入越界");
    }
    let ok = critical_section::with(|cs| {
        let mut guard = FLASH.borrow_ref_mut(cs);
        let flash = guard.as_mut().expect("flash 未初始化");
        flash.erase(from, from + 4096).is_ok() && flash.write_nor(from, buf).is_ok()
    });
    if ok { Ok(()) } else { Err("OTA 扇区写入失败") }
}

/// `nvs` 分区描述。
pub fn nvs_partition() -> Option<Partition> {
    partition(&NVS_PART)
}

/// `openc6_fs` 分区是否可用。
pub fn is_ready() -> bool {
    fs_partition().is_some()
}

/// `openc6_fs` 分区容量（字节），不可用时退回 1 MB。
pub fn fs_size() -> u32 {
    fs_partition().map_or(1024 * 1024, |p| p.size)
}

/// 整片 flash 容量（字节）。
pub fn chip_size() -> u32 {
    CHIP_SIZE.load(core::sync::atomic::Ordering::Relaxed)
}

/// 在 `openc6_fs` 分区内按字节偏移读。
pub fn fs_read(offset: u32, buf: &mut [u8]) {
    let Some(p) = fs_partition() else { return };
    if offset as usize + buf.len() > p.size as usize {
        return;
    }
    critical_section::with(|cs| {
        let mut guard = FLASH.borrow_ref_mut(cs);
        let flash = guard.as_mut().expect("flash 未初始化");
        let _ = esp_storage::FlashStorage::read(flash, p.offset + offset, buf);
    });
}

/// 在 `openc6_fs` 分区内按字节偏移写。
///
/// 用 `write_nor`（不擦除）——日志式文件系统按记录追加，擦整个扇区会带走同扇区的老记录。
/// 调用方负责保证目标区域已经擦过。
pub fn fs_write(offset: u32, buf: &[u8]) {
    let Some(p) = fs_partition() else { return };
    if offset as usize + buf.len() > p.size as usize {
        return;
    }
    critical_section::with(|cs| {
        let mut guard = FLASH.borrow_ref_mut(cs);
        let flash = guard.as_mut().expect("flash 未初始化");
        let base = p.offset + offset;

        // `write_nor` 要求长度是 4 的倍数，否则返回 NotAligned。尾部单独补齐到 4 字节，
        // 补 0xFF 与擦除态一致，不会污染记录内容（真实长度由记录头的 data_len 给出）。
        let tail = buf.len() % 4;
        if tail == 0 {
            if flash.write_nor(base, buf).is_err() {
                defmt::error!("[FLASH] 写入失败: off={:#x} len={}", offset, buf.len());
            }
        } else {
            let head_len = buf.len() - tail;
            if head_len > 0 && flash.write_nor(base, &buf[..head_len]).is_err() {
                defmt::error!("[FLASH] 写入失败: off={:#x} len={}", offset, head_len);
            }
            let mut padded = [0xFFu8; 4];
            padded[..tail].copy_from_slice(&buf[head_len..]);
            if flash
                .write_nor(base + head_len as u32, &padded)
                .is_err()
            {
                defmt::error!("[FLASH] 尾部写入失败: off={:#x}", offset + head_len as u32);
            }
        }
    });
}

/// 擦掉 `openc6_fs` 分区里的第 `sector` 个 4 KB 扇区。
pub fn fs_erase_sector(sector: u32) {
    let Some(p) = fs_partition() else { return };
    let from = p.offset + sector * 4096;
    critical_section::with(|cs| {
        let mut guard = FLASH.borrow_ref_mut(cs);
        let flash = guard.as_mut().expect("flash 未初始化");
        let _ = esp_storage::FlashStorage::erase(flash, from, from + 4096);
    });
}

/// 在 `nvs` 分区内按字节偏移读。
pub fn nvs_read(offset: u32, buf: &mut [u8]) -> bool {
    let Some(p) = nvs_partition() else {
        return false;
    };
    if offset as usize + buf.len() > p.size as usize {
        return false;
    }
    critical_section::with(|cs| {
        let mut guard = FLASH.borrow_ref_mut(cs);
        let flash = guard.as_mut().expect("flash 未初始化");
        esp_storage::FlashStorage::read(flash, p.offset + offset, buf).is_ok()
    })
}

/// 在 `nvs` 分区内擦除并重写一整个 4 KB 扇区。
pub fn nvs_replace_sector(sector: u32, buf: &[u8]) -> bool {
    let Some(p) = nvs_partition() else {
        return false;
    };
    let from = p.offset + sector * 4096;
    if buf.len() > 4096 || from + 4096 > p.offset + p.size {
        return false;
    }
    critical_section::with(|cs| {
        let mut guard = FLASH.borrow_ref_mut(cs);
        let flash = guard.as_mut().expect("flash 未初始化");
        flash.erase(from, from + 4096).is_ok() && flash.write_nor(from, buf).is_ok()
    })
}

