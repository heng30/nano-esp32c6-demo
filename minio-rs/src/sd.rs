//! microSD：SPI 模式（接线见 README：CS=GPIO4、SCK=GPIO3、MISO=GPIO2、MOSI=GPIO1）
//! + embedded-sdmmc FAT 卷。无卡优雅降级：`init` 失败打日志返回 `None`，菜单走纯日志路径。
//! SPI 与 CS 由调用方建好传入（与 OLED 同一模式，引脚 trait 不需要具名）。

use core::cell::RefCell;
use core::ops::ControlFlow;

use critical_section::Mutex;
use defmt::info;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{Mode, SdCard, TimeSource, Timestamp, VolumeIdx, VolumeManager};
use esp_hal::delay::Delay;
use esp_hal::gpio::Output;
use esp_hal::spi::master::Spi;

/// SPI 频率：SD 初始化要求 ≤400 kHz；读写也保持低速（pcap/CSV 落盘量小，稳定优先）
pub const SPI_FREQ_KHZ: u32 = 400;

/// 根目录最多列多少项：防止大卡把堆上的响应体拼爆
const LIST_MAX: usize = 64;

/// SPI 设备（CS 由 ExclusiveDevice 事务级翻转）
type Dev = ExclusiveDevice<Spi<'static, esp_hal::Blocking>, Output<'static>, Delay>;
/// SD 卡块设备
type Card = SdCard<Dev, Delay>;
/// 卷管理器（目录/文件槽各 4、卷 1，embedded-sdmmc 默认上限）
type VolMgr = VolumeManager<Card, ZeroClock>;

/// 空时钟：FAT 时间戳填固定值（板子无 RTC，落盘文件时间戳无意义）
struct ZeroClock;
impl TimeSource for ZeroClock {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 56, // 2026 年
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

/// 已就绪的 SD 会话：卷管理器 + 容量缓存
pub struct Sd {
    mgr: VolMgr,
    bytes: u64,
}

/// 全局 SD 会话：HTTP 文件浏览与菜单共用；无卡时保持 None
static SD: Mutex<RefCell<Option<Sd>>> = Mutex::new(RefCell::new(None));

/// 借出 SD 会话（已挂载卡才有）
pub fn with_sd<R>(f: impl FnOnce(&mut Sd) -> R) -> Option<R> {
    critical_section::with(|cs| {
        let mut guard = SD.borrow_ref_mut(cs);
        guard.as_mut().map(f)
    })
}

/// 初始化 SD 卡并挂载卷 0；`num_bytes` 兼当卡存在性探测
pub fn init(spi: Spi<'static, esp_hal::Blocking>, cs: Output<'static>) -> Result<(), &'static str> {
    let delay = Delay::new();
    let dev = ExclusiveDevice::new(spi, cs, delay).map_err(|_| "spi device init failed")?;
    let card = SdCard::new(dev, delay);
    let bytes = card.num_bytes().map_err(|_| "sd card not found")?;
    let mgr = VolumeManager::new(card, ZeroClock);
    critical_section::with(|cs| {
        SD.borrow_ref_mut(cs).replace(Sd { mgr, bytes });
    });
    Ok(())
}

impl Sd {
    /// 卡容量（字节）
    pub fn capacity_bytes(&self) -> u64 {
        self.bytes
    }

    /// 列出根目录下的文件名与大小，追加到 `out`（Web 文件浏览器用）。
    /// 上限由调用方缓冲控制，避免无界增长。
    pub fn list_root(&self, mut out: impl FnMut(&str, u32)) -> Result<(), &'static str> {
        let vol = self
            .mgr
            .open_volume(VolumeIdx(0))
            .map_err(|_| "open volume failed")?;
        let dir = vol.open_root_dir().map_err(|_| "open root failed")?;
        let mut n = 0;
        dir.iterate_dir(|e| {
            if n >= LIST_MAX {
                return ControlFlow::Break(());
            }
            if !e.attributes.is_directory() {
                out(alloc::format!("{}", e.name).as_str(), e.size);
                n += 1;
            }
            ControlFlow::Continue(())
        })
        .map_err(|_| "iterate dir failed")
    }

    /// 新建/覆盖一个文件并写入全部字节（一次性落盘：自检、wardriving 快照用）。
    /// 活的 pcap 流式追加等接线验证后再接（VolumeManager 文件槽长期占用需评估）。
    pub fn write_new_file(&self, name: &str, data: &[u8]) -> Result<(), &'static str> {
        // 借用链每次重建：volume → root → file，写完 Drop 释放文件槽
        let vol = self
            .mgr
            .open_volume(VolumeIdx(0))
            .map_err(|_| "open volume failed")?;
        let dir = vol.open_root_dir().map_err(|_| "open root failed")?;
        let file = dir
            .open_file_in_dir(name, Mode::ReadWriteCreate)
            .map_err(|_| "create file failed")?;
        file.write(data).map_err(|_| "write failed")?;
        info!("sd file written: {} bytes", data.len());
        Ok(())
    }
}
