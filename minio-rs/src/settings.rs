//! 偏好持久化：`nvs` 分区第 0 扇区存线格式配置块（CRC32 校验，布局见 core::config）。
//! 写入前整扇区擦除；无 nvs 分区时退化为只改内存。

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};

use critical_section::Mutex;
use esp_storage::FlashStorage;
use minio_rs_core::config::{self, Config};

/// 分区表读入缓冲长度（C6 固定 0xC00）
const PARTITION_TABLE_SIZE: usize = 0xC00;
/// 配置块占用的扇区大小
const SECTOR: u32 = 4096;

static FLASH: Mutex<RefCell<Option<FlashStorage<'static>>>> = Mutex::new(RefCell::new(None));
/// nvs 分区 (offset, size)；`init` 之后才有值
static NVS: Mutex<RefCell<Option<(u32, u32)>>> = Mutex::new(RefCell::new(None));
/// 配置缓存；`init` 之后才有值
static CACHE: Mutex<RefCell<Option<Config>>> = Mutex::new(RefCell::new(None));
/// flash 里是否已有通过校验的配置
static PROVISIONED: AtomicBool = AtomicBool::new(false);

/// 初始化 flash、解析分区表找 `nvs`、加载配置。
pub fn init(flash_peripheral: esp_hal::peripherals::FLASH<'static>) {
    let flash = FlashStorage::new(flash_peripheral);
    critical_section::with(|cs| FLASH.borrow_ref_mut(cs).replace(flash));

    let mut buf = [0u8; PARTITION_TABLE_SIZE];
    let table = critical_section::with(|cs| {
        let mut guard = FLASH.borrow_ref_mut(cs);
        let flash = guard.as_mut().expect("flash not initialized");
        esp_bootloader_esp_idf::partitions::read_partition_table(flash, &mut buf)
    });

    let mut nvs = None;
    match table {
        Ok(table) => {
            for entry in table.iter() {
                if entry.label_as_str() == "nvs" {
                    nvs = Some((entry.offset(), entry.len()));
                }
            }
        }
        Err(_) => defmt::error!("partition table read failed"),
    }

    let mut loaded: Option<Config> = None;
    if let Some((off, size)) = nvs {
        if size as usize >= config::RAW_SIZE {
            let mut raw = [0u8; config::RAW_SIZE];
            let ok = critical_section::with(|cs| {
                let mut guard = FLASH.borrow_ref_mut(cs);
                let flash = guard.as_mut().expect("flash not initialized");
                esp_storage::FlashStorage::read(flash, off, &mut raw).is_ok()
            });
            if ok {
                loaded = config::decode(&raw);
            }
        }
    }

    PROVISIONED.store(loaded.is_some(), Ordering::Relaxed);
    critical_section::with(|cs| {
        *NVS.borrow_ref_mut(cs) = nvs;
        *CACHE.borrow_ref_mut(cs) = Some(loaded.unwrap_or_default());
    });
    match (nvs, loaded) {
        (Some(_), Some(_)) => defmt::info!("settings: loaded from nvs"),
        (Some(_), None) => defmt::info!("settings: defaults (first boot or bad crc)"),
        (None, _) => defmt::warn!("no nvs partition, settings not persisted"),
    }
}

/// 借出 flash 实例供其它模块复用（`FlashStorage::new` 只能调一次）
pub fn with_flash<R>(f: impl FnOnce(&mut FlashStorage<'static>) -> R) -> Option<R> {
    critical_section::with(|cs| {
        let mut guard = FLASH.borrow_ref_mut(cs);
        guard.as_mut().map(f)
    })
}

/// 当前配置快照；`init` 前调用返回默认值
pub fn current() -> Config {
    critical_section::with(|cs| CACHE.borrow_ref(cs).unwrap_or_default())
}

/// 修改配置并立即落盘；无 nvs 分区时只改内存
pub fn update(mutator: impl FnOnce(&mut Config)) {
    let cfg = critical_section::with(|cs| {
        let mut c = CACHE.borrow_ref_mut(cs);
        mutator(c.as_mut().expect("settings not initialized"));
        *c
    })
    .unwrap_or_default();
    let mut raw = [0u8; config::RAW_SIZE];
    config::encode(&cfg, &mut raw);
    let ok = critical_section::with(|cs| {
        let Some((off, size)) = *NVS.borrow_ref(cs) else {
            return false;
        };
        if size < SECTOR {
            return false;
        }
        let mut guard = FLASH.borrow_ref_mut(cs);
        let Some(flash) = guard.as_mut() else {
            return false;
        };
        flash.erase(off, off + SECTOR).is_ok() && flash.write_nor(off, &raw).is_ok()
    });
    if ok {
        PROVISIONED.store(true, Ordering::Relaxed);
        defmt::info!("settings saved");
    } else {
        defmt::warn!("settings save failed (no nvs or flash error)");
    }
}
