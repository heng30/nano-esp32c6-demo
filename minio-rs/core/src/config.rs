//! 偏好配置：菜单风格 / 隐身模式 / 屏保超时 / STA 凭据。
//! 字节级线格式（无 repr(C) 布局歧义），CRC32 校验，固件侧落到 nvs 分区第 0 扇区。

/// OLED 菜单风格，对应上游 Classic / Icon List / Grid / Hero+Dock
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuStyle {
    Classic,
    IconList,
    Grid,
    HeroDock,
}

impl MenuStyle {
    /// 显示名
    pub fn name(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::IconList => "Icon List",
            Self::Grid => "Grid",
            Self::HeroDock => "Hero+Dock",
        }
    }

    /// 短按切换到下一风格，循环
    pub fn cycle(self) -> Self {
        match self {
            Self::Classic => Self::IconList,
            Self::IconList => Self::Grid,
            Self::Grid => Self::HeroDock,
            Self::HeroDock => Self::Classic,
        }
    }
}

/// SSID 定长容量（含结尾 0）
pub const SSID_MAX: usize = 33;
/// 密码定长容量（含结尾 0）
pub const PASS_MAX: usize = 65;

/// 屏保超时档位；0 = 关闭
pub const SCREENSAVER_STEPS: [u16; 4] = [15, 30, 60, 0];

/// 一整套偏好
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// OLED 菜单风格
    pub menu_style: MenuStyle,
    /// 隐身模式：灭灯/无声/无动画
    pub stealth: bool,
    /// 屏保超时秒数，0 = 关闭
    pub screensaver_secs: u16,
    /// STA SSID，结尾 0
    pub sta_ssid: [u8; SSID_MAX],
    /// STA 密码，结尾 0
    pub sta_pass: [u8; PASS_MAX],
}

impl Default for Config {
    fn default() -> Self {
        Self {
            menu_style: MenuStyle::Classic,
            stealth: false,
            screensaver_secs: 30,
            sta_ssid: [0; SSID_MAX],
            sta_pass: [0; PASS_MAX],
        }
    }
}

impl Config {
    /// SSID 字符串视图
    pub fn ssid_str(&self) -> &str {
        from_fixed(&self.sta_ssid)
    }

    /// 密码字符串视图
    pub fn pass_str(&self) -> &str {
        from_fixed(&self.sta_pass)
    }

    /// 写入 STA 凭据；空 SSID 表示清除
    pub fn set_wifi_sta(&mut self, ssid: &str, pass: &str) {
        self.sta_ssid = to_fixed(ssid);
        self.sta_pass = to_fixed(pass);
    }
}

/// 把字符串截断写进定长字节数组，保证结尾有 0
fn to_fixed<const N: usize>(s: &str) -> [u8; N] {
    let mut out = [0u8; N];
    let bytes = s.as_bytes();
    let n = bytes.len().min(N - 1);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

/// 从定长字节数组取字符串，遇 0 截断
fn from_fixed(bytes: &[u8]) -> &str {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..end]).unwrap_or("")
}

/// 线格式字节数；写入长度须是 4 的倍数（write_nor 对齐要求）
pub const RAW_SIZE: usize = 116;

/// 魔数（ASCII "MINV"）
const MAGIC: u32 = 0x4D49_4E56;
/// 布局版本，字段变动时自增
const VERSION: u32 = 1;

/// 各字段在线格式里的偏移
mod off {
    pub const MAGIC: usize = 0;
    pub const VERSION: usize = 4;
    pub const CRC: usize = 8;
    pub const BODY: usize = 12;
    pub const STYLE: usize = 12;
    pub const STEALTH: usize = 13;
    pub const SECS: usize = 14;
    pub const SSID: usize = 16;
    pub const PASS: usize = 16 + super::SSID_MAX;
}

/// CRC32（IEEE 802.3，按位实现，块只有百来字节）
pub fn crc32(data: &[u8]) -> u32 {
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

/// 编码到线格式字节
pub fn encode(cfg: &Config, out: &mut [u8; RAW_SIZE]) {
    out.fill(0);
    out[off::MAGIC..off::MAGIC + 4].copy_from_slice(&MAGIC.to_le_bytes());
    out[off::VERSION..off::VERSION + 4].copy_from_slice(&VERSION.to_le_bytes());
    out[off::STYLE] = match cfg.menu_style {
        MenuStyle::Classic => 0,
        MenuStyle::IconList => 1,
        MenuStyle::Grid => 2,
        MenuStyle::HeroDock => 3,
    };
    out[off::STEALTH] = cfg.stealth as u8;
    out[off::SECS..off::SECS + 2].copy_from_slice(&cfg.screensaver_secs.to_le_bytes());
    out[off::SSID..off::SSID + SSID_MAX].copy_from_slice(&cfg.sta_ssid);
    out[off::PASS..off::PASS + PASS_MAX].copy_from_slice(&cfg.sta_pass);
    let crc = crc32(&out[off::BODY..]);
    out[off::CRC..off::CRC + 4].copy_from_slice(&crc.to_le_bytes());
}

/// 解码线格式字节；魔数/版本/CRC 任一不对返回 None
pub fn decode(bytes: &[u8; RAW_SIZE]) -> Option<Config> {
    let magic = u32::from_le_bytes(bytes[off::MAGIC..off::MAGIC + 4].try_into().ok()?);
    if magic != MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(bytes[off::VERSION..off::VERSION + 4].try_into().ok()?);
    if version != VERSION {
        return None;
    }
    let crc = u32::from_le_bytes(bytes[off::CRC..off::CRC + 4].try_into().ok()?);
    if crc != crc32(&bytes[off::BODY..]) {
        return None;
    }
    let menu_style = match bytes[off::STYLE] {
        0 => MenuStyle::Classic,
        1 => MenuStyle::IconList,
        2 => MenuStyle::Grid,
        _ => MenuStyle::HeroDock,
    };
    let mut sta_ssid = [0u8; SSID_MAX];
    sta_ssid.copy_from_slice(&bytes[off::SSID..off::SSID + SSID_MAX]);
    let mut sta_pass = [0u8; PASS_MAX];
    sta_pass.copy_from_slice(&bytes[off::PASS..off::PASS + PASS_MAX]);
    Some(Config {
        menu_style,
        stealth: bytes[off::STEALTH] != 0,
        screensaver_secs: u16::from_le_bytes(bytes[off::SECS..off::SECS + 2].try_into().ok()?),
        sta_ssid,
        sta_pass,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_size_is_word_aligned() {
        // write_nor 要求 4 字节倍数
        assert_eq!(RAW_SIZE % 4, 0);
    }

    #[test]
    fn roundtrip_default_and_custom() {
        let cfg = Config::default();
        let mut buf = [0u8; RAW_SIZE];
        encode(&cfg, &mut buf);
        assert_eq!(decode(&buf).unwrap(), cfg);

        let mut cfg = Config::default();
        cfg.menu_style = MenuStyle::Grid;
        cfg.stealth = true;
        cfg.screensaver_secs = 60;
        cfg.set_wifi_sta("ow-blue", "12345678");
        encode(&cfg, &mut buf);
        assert_eq!(decode(&buf).unwrap(), cfg);
    }

    #[test]
    fn bad_magic_rejected() {
        let mut buf = [0u8; RAW_SIZE];
        encode(&Config::default(), &mut buf);
        buf[0] ^= 0xFF;
        assert!(decode(&buf).is_none());
    }

    #[test]
    fn tampered_body_rejected_by_crc() {
        let mut buf = [0u8; RAW_SIZE];
        encode(&Config::default(), &mut buf);
        // 改一个正文位但不改 CRC
        buf[off::STEALTH] ^= 1;
        assert!(decode(&buf).is_none());
    }

    #[test]
    fn ssid_truncated_and_recovered() {
        // 超长 SSID 截断到定长容量-1
        let long = [b'x'; 64];
        let long_ssid = core::str::from_utf8(&long).unwrap();
        let mut cfg = Config::default();
        cfg.set_wifi_sta(long_ssid, "p");
        assert_eq!(cfg.ssid_str().len(), SSID_MAX - 1);
        let mut buf = [0u8; RAW_SIZE];
        encode(&cfg, &mut buf);
        assert_eq!(decode(&buf).unwrap().ssid_str().len(), SSID_MAX - 1);
    }

    #[test]
    fn style_cycles_all_four() {
        let mut s = MenuStyle::Classic;
        let mut seen = 0;
        loop {
            s = s.cycle();
            seen += 1;
            if s == MenuStyle::Classic {
                break;
            }
        }
        assert_eq!(seen, 4);
    }
}
