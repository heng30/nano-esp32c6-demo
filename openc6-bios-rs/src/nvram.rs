//! CMOS NVRAM：BIOS 配置的持久化存储。
//!
//! 上游用 ESP-IDF 的 NVS 键值库，这里改成固定布局的结构体 + CRC32 存在 `nvs` 分区
//! 第 0 扇区。整块配置只有 239 字节，键值库的收益抵不上重写一遍 blob 格式的成本。

use core::cell::RefCell;

use critical_section::Mutex;

use crate::hal::flash;

/// 配置块魔数（ASCII "OCNV"）。
const MAGIC: u32 = 0x4F43_4E56;
/// 配置块布局版本，字段变动时自增，旧版本自动回落到默认值。
const VERSION: u32 = 1;
/// 配置块在 flash 里的实际字节数，给 Web UI 算占用率用。
pub const CONFIG_BYTES: usize = core::mem::size_of::<RawConfig>();

/// 掉电后是否自动开机。
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum DcLossAction {
    /// 保持关机。
    PowerOff,
    /// 自动开机。
    PowerOn,
}

/// POST 阶段是否闪 LED。
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum PostLedMode {
    /// 不闪。
    Disabled,
    /// 闪。
    Enabled,
}

/// Aura Sync 灯效档位，数值与 Web UI 的下拉索引一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum AuraMode {
    /// 静态蓝。
    Static,
    /// 彩虹流动。
    Rainbow,
    /// 关闭。
    Disabled,
}

/// CPU 频率档位（MHz）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum CpuFreq {
    /// 80 MHz。
    Mhz80,
    /// 120 MHz。
    Mhz120,
    /// 160 MHz。
    Mhz160,
}

impl CpuFreq {
    /// 转成 MHz 数值。
    pub fn mhz(self) -> u32 {
        match self {
            Self::Mhz80 => 80,
            Self::Mhz120 => 120,
            Self::Mhz160 => 160,
        }
    }
}

/// 调频策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum CpuGovernor {
    /// 锁在 NVRAM 设定的频率。
    Performance,
    /// 按负载动态调频。
    Dynamic,
}

/// 欠压检测阈值档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum BodLevel {
    /// 2.8V 严格档。
    Strict,
    /// 2.5V 宽松档。
    Relaxed,
    /// 关闭欠压检测。
    Disabled,
}

/// ZC6 压缩与 ZSWAP 模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum Zc6SwapMode {
    /// 全关，只跑原始 .bin。
    Disabled,
    /// 只透明解压 flash 上的 .zc6。
    Flash,
    /// 只做挂起进程的 RAM 压缩。
    Ram,
    /// 两者都开。
    Both,
}

impl Zc6SwapMode {
    /// flash 上的 `.zc6` 是否需要透明解压。
    pub fn flash_enabled(self) -> bool {
        matches!(self, Self::Flash | Self::Both)
    }
}

/// BIOS OTA 更新状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum BiosUpdateState {
    /// 无待处理更新。
    None,
    /// 有待执行的无线更新。
    Pending,
}

/// 远程 Web Shell（c6wsh）开关。
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum C6wshState {
    /// 关闭，RF 基带不上电，省下约 90 KB SRAM 给 payload。
    Disabled,
    /// 开启，POST 时连 Wi-Fi 并起 c6wsh 守护进程。
    Enabled,
}

/// Wi-Fi 国家码最大长度（含结尾 0）。
pub const WIFI_COUNTRY_MAX_LEN: usize = 3;
/// Wi-Fi SSID 最大长度。
pub const WIFI_SSID_MAX_LEN: usize = 32;
/// Wi-Fi 密码最大长度。
pub const WIFI_PASS_MAX_LEN: usize = 64;
/// PXE URL 最大长度。
pub const PXE_URL_MAX_LEN: usize = 128;

/// 默认 Wi-Fi 国家码：01 = World Safe Mode，信道 1–13。
pub const DEFAULT_WIFI_COUNTRY: &str = "01";
/// 默认 PXE 地址。
pub const DEFAULT_PXE_URL: &str = "http://192.168.1.1:8080/payload.bin";
/// 默认降频温度阈值（摄氏度）。
pub const DEFAULT_THROTTLE_C: u8 = 55;
/// 默认紧急关机温度阈值（摄氏度）。
pub const DEFAULT_EMERGENCY_C: u8 = 75;

/// 一整套 BIOS 配置。
#[derive(Debug, Clone, Copy, defmt::Format)]
pub struct Config {
    /// 掉电恢复策略。
    pub dc_loss_action: DcLossAction,
    /// POST 灯效开关。
    pub post_led_mode: PostLedMode,
    /// Aura 灯效档位。
    pub aura_mode: AuraMode,
    /// Aura 亮度 0–255。
    pub aura_brightness: u8,
    /// CPU 频率档位。
    pub cpu_freq: CpuFreq,
    /// 调频策略。
    pub cpu_governor: CpuGovernor,
    /// 欠压检测档位。
    pub bod_level: BodLevel,
    /// ZC6 压缩模式。
    pub zc6_swap_mode: Zc6SwapMode,
    /// 降频温度阈值（摄氏度）。
    pub thermal_throttle_c: u8,
    /// 紧急关机温度阈值（摄氏度）。
    pub thermal_emergency_c: u8,
    /// OTA 更新状态。
    pub bios_update_state: BiosUpdateState,
    /// c6wsh 开关。
    pub c6wsh_state: C6wshState,
    /// Wi-Fi 国家码，2 字母 + 结尾 0。
    pub wifi_country: [u8; WIFI_COUNTRY_MAX_LEN],
    /// Wi-Fi STA SSID，结尾 0。
    pub wifi_ssid: [u8; WIFI_SSID_MAX_LEN],
    /// Wi-Fi STA 密码，结尾 0。
    pub wifi_pass: [u8; WIFI_PASS_MAX_LEN],
    /// PXE 目标 URL，结尾 0。
    pub pxe_url: [u8; PXE_URL_MAX_LEN],
}

impl Default for Config {
    fn default() -> Self {
        let mut cfg = Self {
            dc_loss_action: DcLossAction::PowerOn,
            post_led_mode: PostLedMode::Enabled,
            aura_mode: AuraMode::Rainbow,
            aura_brightness: 128,
            cpu_freq: CpuFreq::Mhz160,
            cpu_governor: CpuGovernor::Dynamic,
            bod_level: BodLevel::Strict,
            zc6_swap_mode: Zc6SwapMode::Both,
            thermal_throttle_c: DEFAULT_THROTTLE_C,
            thermal_emergency_c: DEFAULT_EMERGENCY_C,
            bios_update_state: BiosUpdateState::None,
            c6wsh_state: C6wshState::Disabled,
            wifi_country: [0; WIFI_COUNTRY_MAX_LEN],
            wifi_ssid: [0; WIFI_SSID_MAX_LEN],
            wifi_pass: [0; WIFI_PASS_MAX_LEN],
            pxe_url: [0; PXE_URL_MAX_LEN],
        };
        cfg.wifi_country = to_fixed(DEFAULT_WIFI_COUNTRY);
        cfg.pxe_url = to_fixed(DEFAULT_PXE_URL);
        cfg
    }
}

impl Config {
    /// SSID 字符串视图。
    pub fn ssid_str(&self) -> &str {
        from_fixed(&self.wifi_ssid)
    }

    /// 密码字符串视图。
    pub fn pass_str(&self) -> &str {
        from_fixed(&self.wifi_pass)
    }

    /// 国家码字符串视图。
    pub fn country_str(&self) -> &str {
        from_fixed(&self.wifi_country)
    }

    /// PXE URL 字符串视图。
    pub fn pxe_url_str(&self) -> &str {
        from_fixed(&self.pxe_url)
    }

    /// 写入 SSID / 密码。
    pub fn set_wifi_sta(&mut self, ssid: &str, pass: &str) {
        self.wifi_ssid = to_fixed(ssid);
        self.wifi_pass = to_fixed(pass);
    }

    /// 写入 PXE URL。
    pub fn set_pxe_url(&mut self, url: &str) {
        self.pxe_url = to_fixed(url);
    }

    /// 写入 Wi-Fi 国家码。
    pub fn set_wifi_country(&mut self, country: &str) {
        self.wifi_country = to_fixed(country);
    }
}

/// 把字符串截断写进定长字节数组，保证结尾有 0。
fn to_fixed<const N: usize>(s: &str) -> [u8; N] {
    let mut out = [0u8; N];
    let bytes = s.as_bytes();
    let n = bytes.len().min(N - 1);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

/// 从定长字节数组取字符串，遇 0 截断。
fn from_fixed(bytes: &[u8]) -> &str {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..end]).unwrap_or("")
}

/// 落到 flash 上的线格式。
#[repr(C)]
#[derive(Clone, Copy)]
struct RawConfig {
    magic: u32,
    version: u32,
    crc: u32,
    dc_loss_action: u8,
    post_led_mode: u8,
    aura_mode: u8,
    aura_brightness: u8,
    cpu_freq_mhz: u8,
    cpu_governor: u8,
    bod_level: u8,
    zc6_swap_mode: u8,
    thermal_throttle_c: u8,
    thermal_emergency_c: u8,
    bios_update_state: u8,
    c6wsh_state: u8,
    wifi_country: [u8; WIFI_COUNTRY_MAX_LEN],
    wifi_ssid: [u8; WIFI_SSID_MAX_LEN],
    wifi_pass: [u8; WIFI_PASS_MAX_LEN],
    pxe_url: [u8; PXE_URL_MAX_LEN],
}

/// 配置缓存。
static CACHE: Mutex<RefCell<Option<Config>>> = Mutex::new(RefCell::new(None));
/// flash 里是否存在过一份通过校验的配置。
static PROVISIONED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// 计算 CRC32（IEEE 802.3，按位实现，配置块只有 239 字节）。
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

impl RawConfig {
    fn to_config(&self) -> Config {
        Config {
            dc_loss_action: if self.dc_loss_action == 1 {
                DcLossAction::PowerOn
            } else {
                DcLossAction::PowerOff
            },
            post_led_mode: if self.post_led_mode == 1 {
                PostLedMode::Enabled
            } else {
                PostLedMode::Disabled
            },
            aura_mode: match self.aura_mode {
                0 => AuraMode::Static,
                2 => AuraMode::Disabled,
                _ => AuraMode::Rainbow,
            },
            aura_brightness: self.aura_brightness,
            cpu_freq: match self.cpu_freq_mhz {
                80 => CpuFreq::Mhz80,
                120 => CpuFreq::Mhz120,
                _ => CpuFreq::Mhz160,
            },
            cpu_governor: if self.cpu_governor == 1 {
                CpuGovernor::Dynamic
            } else {
                CpuGovernor::Performance
            },
            bod_level: match self.bod_level {
                1 => BodLevel::Relaxed,
                2 => BodLevel::Disabled,
                _ => BodLevel::Strict,
            },
            zc6_swap_mode: match self.zc6_swap_mode {
                0 => Zc6SwapMode::Disabled,
                1 => Zc6SwapMode::Flash,
                2 => Zc6SwapMode::Ram,
                _ => Zc6SwapMode::Both,
            },
            thermal_throttle_c: self.thermal_throttle_c,
            thermal_emergency_c: self.thermal_emergency_c,
            bios_update_state: if self.bios_update_state == 1 {
                BiosUpdateState::Pending
            } else {
                BiosUpdateState::None
            },
            c6wsh_state: if self.c6wsh_state == 1 {
                C6wshState::Enabled
            } else {
                C6wshState::Disabled
            },
            wifi_country: self.wifi_country,
            wifi_ssid: self.wifi_ssid,
            wifi_pass: self.wifi_pass,
            pxe_url: self.pxe_url,
        }
    }

    fn from_config(cfg: &Config) -> Self {
        let mut raw = Self {
            magic: MAGIC,
            version: VERSION,
            crc: 0,
            dc_loss_action: matches!(cfg.dc_loss_action, DcLossAction::PowerOn) as u8,
            post_led_mode: matches!(cfg.post_led_mode, PostLedMode::Enabled) as u8,
            aura_mode: match cfg.aura_mode {
                AuraMode::Static => 0,
                AuraMode::Rainbow => 1,
                AuraMode::Disabled => 2,
            },
            aura_brightness: cfg.aura_brightness,
            cpu_freq_mhz: cfg.cpu_freq.mhz() as u8,
            cpu_governor: matches!(cfg.cpu_governor, CpuGovernor::Dynamic) as u8,
            bod_level: match cfg.bod_level {
                BodLevel::Strict => 0,
                BodLevel::Relaxed => 1,
                BodLevel::Disabled => 2,
            },
            zc6_swap_mode: match cfg.zc6_swap_mode {
                Zc6SwapMode::Disabled => 0,
                Zc6SwapMode::Flash => 1,
                Zc6SwapMode::Ram => 2,
                Zc6SwapMode::Both => 3,
            },
            thermal_throttle_c: cfg.thermal_throttle_c,
            thermal_emergency_c: cfg.thermal_emergency_c,
            bios_update_state: matches!(cfg.bios_update_state, BiosUpdateState::Pending) as u8,
            c6wsh_state: matches!(cfg.c6wsh_state, C6wshState::Enabled) as u8,
            wifi_country: cfg.wifi_country,
            wifi_ssid: cfg.wifi_ssid,
            wifi_pass: cfg.wifi_pass,
            pxe_url: cfg.pxe_url,
        };
        raw.crc = crc32(raw.body());
        raw
    }

    /// 参与 CRC 的字节：跳过 `magic`、`version` 和 `crc` 三个 u32。
    ///
    /// 必须从 12 开始——如果从 8 开始会把 `crc` 字段本身也算进去，保存时它还是 0、
    /// 加载时已是校验值，两次结果必然不同，等于每次启动都判定配置无效。
    fn body(&self) -> &[u8] {
        let ptr = self as *const Self as *const u8;
        let len = core::mem::size_of::<Self>();
        unsafe { core::slice::from_raw_parts(ptr.add(12), len - 12) }
    }

    fn as_bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self as *const Self as *const u8, core::mem::size_of::<Self>()) }
    }
}

/// 挂载配置：flash 里有合法块就读出来，否则写入默认值。
pub fn init() {
    let mut buf = [0u8; core::mem::size_of::<RawConfig>()];
    let loaded = if flash::nvs_read(0, &mut buf) {
        let raw: RawConfig = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const RawConfig) };
        if raw.magic == MAGIC && raw.version == VERSION && raw.crc == crc32(raw.body()) {
            Some(raw.to_config())
        } else {
            None
        }
    } else {
        None
    };

    match loaded {
        Some(cfg) => {
            defmt::info!("NVRAM 已加载 (aura={}, cpu={} MHz)", cfg.aura_mode, cfg.cpu_freq.mhz());
            critical_section::with(|cs| *CACHE.borrow_ref_mut(cs) = Some(cfg));
            PROVISIONED.store(true, core::sync::atomic::Ordering::Release);
        }
        None => {
            defmt::warn!("NVRAM 无有效配置，写入默认值");
            load_defaults();
        }
    }
}

/// flash 里是否已经有一份通过校验的配置。
///
/// 出厂第一次上电时是 `false`，此时不该执行任何"掉电恢复"策略——否则新板子插上电
/// 就把自己关进 S5，用户只会看到一块死板。
pub fn is_provisioned() -> bool {
    PROVISIONED.load(core::sync::atomic::Ordering::Acquire)
}

/// 恢复出厂设置（Clear CMOS）。
pub fn load_defaults() {
    let cfg = Config::default();
    critical_section::with(|cs| *CACHE.borrow_ref_mut(cs) = Some(cfg));
    persist(&cfg);
}

/// 读当前配置的副本。
pub fn get() -> Config {
    critical_section::with(|cs| CACHE.borrow_ref(cs).unwrap_or_default())
}

/// 改配置并落盘。
pub fn update(f: impl FnOnce(&mut Config)) {
    let mut cfg = get();
    f(&mut cfg);
    critical_section::with(|cs| *CACHE.borrow_ref_mut(cs) = Some(cfg));
    persist(&cfg);
}

/// 只改内存不落盘，供 POST 期间临时覆盖。
fn persist(cfg: &Config) {
    let raw = RawConfig::from_config(cfg);
    let mut buf = [0xFFu8; 4096];
    let bytes = raw.as_bytes();
    buf[..bytes.len()].copy_from_slice(bytes);
    if !flash::nvs_replace_sector(0, &buf) {
        defmt::error!("NVRAM 写入失败");
    }
}
