//! BLE 跟踪器/监控设备指纹分类：从广播载荷识别 AirTag / SmartTag / Tile / Chipolo / FMDN。
//!
//! 签名依据（公开逆向资料核实）：
//! - Apple Find My：制造商数据 0x004C + Apple type `0x12`；长型（len `0x19`）为分离态，
//!   状态字节高位是粗电量、bit5 是“在主人身边”提示——只匹配 0x004C 不行，
//!   所有苹果设备都在发；
//! - Tile：服务数据 UUID `0xFEED`（已激活）/`0xFEEC`（未激活）/`0xFE84`（旧）；无分离信号；
//! - Samsung SmartTag：服务数据 UUID `0xFD5A`（公司 ID 0x0075 不是判据，所有三星设备都有），
//!   首字节 bit5–7 是状态：2/3 = 分离；
//! - Chipolo：服务数据 UUID `0xFE33`；公司 `0x0157` 的 ONE Spot 走 Find My 网络；
//! - Google FMDN：服务数据 UUID `0xFEAA`、帧类型 `0x40`/`0x41`（0x41 = 分离）。

/// 跟踪器生态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackerKind {
    /// Apple Find My（AirTag 及接入 Find My 的配件）
    AppleFindMy,
    /// Tile（Mate/Pro/Slim）
    Tile,
    /// Samsung Galaxy SmartTag / SmartTag2
    SamsungSmartTag,
    /// Chipolo（含走 Find My 的 ONE Spot）
    Chipolo,
    /// Google Find My Device Network
    GoogleFmdn,
}

impl TrackerKind {
    /// 日志用短名
    pub fn name(self) -> &'static str {
        match self {
            Self::AppleFindMy => "apple-findmy",
            Self::Tile => "tile",
            Self::SamsungSmartTag => "samsung-smarttag",
            Self::Chipolo => "chipolo",
            Self::GoogleFmdn => "google-fmdn",
        }
    }
}

/// 分类结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackerHit {
    /// 生态
    pub kind: TrackerKind,
    /// 是否分离态广播；Tile/Chipolo 无此信号为 None
    pub separated: Option<bool>,
    /// 粗电量；仅 Find My 状态字节携带
    pub battery: Option<&'static str>,
}

/// 苹果公司 ID
const APPLE: u16 = 0x004C;
/// Chipolo ONE Spot（走 Find My 网络的型号）
const COPOLO_ONE_SPOT: u16 = 0x0157;
/// Find My 分离态的 Apple 数据长度
const FINDMY_SEPARATED_LEN: u8 = 0x19;

/// 遍历广播里的 AD 结构，返回第一个跟踪器命中
pub fn classify_adv(adv: &[u8]) -> Option<TrackerHit> {
    let mut pos = 0usize;
    while pos + 1 < adv.len() {
        let len = adv[pos] as usize;
        if len == 0 {
            break;
        }
        // len 含类型字节；数据长度 = len - 1
        let ty = adv[pos + 1];
        let end = (pos + 1 + len).min(adv.len());
        let data = adv.get(pos + 2..end)?;
        if let Some(hit) = match_ad(ty, data) {
            return Some(hit);
        }
        pos += 1 + len;
    }
    None
}

/// 单个 AD 结构的分类
fn match_ad(ty: u8, data: &[u8]) -> Option<TrackerHit> {
    match ty {
        // 0xFF 制造商特定数据
        0xFF if data.len() >= 3 => {
            let company = u16::from_le_bytes([data[0], data[1]]);
            if company == APPLE && data[2] == 0x12 {
                // Apple data = [type][len][status][key...]
                let separated_len = data.len() >= 4 && data[3] == FINDMY_SEPARATED_LEN;
                let (separated, battery) = if data.len() >= 5 {
                    let status = data[4];
                    // bit5 = 在主人身边；分离态长型才可信
                    let maintained = status & 0x20 != 0;
                    let battery = match status >> 6 {
                        0 => "critical",
                        1 => "low",
                        2 => "medium",
                        _ => "full",
                    };
                    (Some(separated_len && !maintained), Some(battery))
                } else {
                    (Some(false), None)
                };
                return Some(TrackerHit { kind: TrackerKind::AppleFindMy, separated, battery });
            }
            if company == COPOLO_ONE_SPOT {
                return Some(TrackerHit {
                    kind: TrackerKind::Chipolo,
                    separated: None,
                    battery: None,
                });
            }
            None
        }
        // 0x16 服务数据（16 位 UUID）
        0x16 if data.len() >= 3 => {
            let uuid = u16::from_le_bytes([data[0], data[1]]);
            match uuid {
                // Tile：无分离信号
                0xFEED | 0xFEEC | 0xFE84 => Some(TrackerHit {
                    kind: TrackerKind::Tile,
                    separated: None,
                    battery: None,
                }),
                // SmartTag：首字节 bit5–7 状态，2/3 = 分离
                0xFD5A => {
                    let state = data[2] >> 5;
                    let separated = match state {
                        2 | 3 => Some(true),
                        4..=6 => Some(false),
                        _ => None,
                    };
                    Some(TrackerHit {
                        kind: TrackerKind::SamsungSmartTag,
                        separated,
                        battery: None,
                    })
                }
                0xFE33 => Some(TrackerHit {
                    kind: TrackerKind::Chipolo,
                    separated: None,
                    battery: None,
                }),
                // FMDN：帧类型 0x41 = 分离
                0xFEAA if data[2] == 0x40 || data[2] == 0x41 => Some(TrackerHit {
                    kind: TrackerKind::GoogleFmdn,
                    separated: Some(data[2] == 0x41),
                    battery: None,
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// 监控设备名指纹（surveillance 模式）：对 0x09 完整本地名做子串匹配。
/// 上游的 WiFi 侧指纹（Flock OUI 等）被 blob 挡死，这里只有 BLE 名字这一小片。
pub fn surveillance_name(adv: &[u8]) -> Option<&'static str> {
    let mut pos = 0usize;
    while pos + 1 < adv.len() {
        let len = adv[pos] as usize;
        if len == 0 {
            break;
        }
        let ty = adv[pos + 1];
        let end = (pos + 1 + len).min(adv.len());
        if ty == 0x09 {
            if let Ok(name) = core::str::from_utf8(adv.get(pos + 2..end).unwrap_or(&[])) {
                if contains_ci(name, "axon") {
                    return Some("axon body cam");
                }
                if contains_ci(name, "vending") || contains_ci(name, "free ") {
                    return Some("possible skimmer");
                }
            }
        }
        pos += 1 + len;
    }
    None
}

/// 大小写不敏感子串匹配（no_std 无 to_lowercase，逐窗口比对）
fn contains_ci(hay: &str, needle: &str) -> bool {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return false;
    }
    h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

#[cfg(test)]
mod tests {
    // 测试用 Vec，宿主构建下引入 std
    extern crate std;
    use std::vec::Vec;
    use std::vec;

    use super::*;

    /// 拼一个 AD 结构：`[len][ty][data...]`
    fn ad(ty: u8, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(data.len() + 2);
        v.push((data.len() + 1) as u8);
        v.push(ty);
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn findmy_separated_long_form() {
        // 长型（len 0x19）+ 状态 bit5 清零 = 分离态
        let mut data = vec![0x4C, 0x00, 0x12, 0x19, 0x00];
        data.extend_from_slice(&[0xAA; 24]);
        let adv = ad(0xFF, &data);
        let hit = classify_adv(&adv).unwrap();
        assert_eq!(hit.kind, TrackerKind::AppleFindMy);
        assert_eq!(hit.separated, Some(true));
        assert_eq!(hit.battery, Some("critical"));
    }

    #[test]
    fn findmy_maintained_short_form() {
        // 短型 + bit5 置位 = 在主人身边，不是威胁
        let data = [0x4C, 0x00, 0x12, 0x0A, 0x20];
        let adv = ad(0xFF, &data);
        let hit = classify_adv(&adv).unwrap();
        assert_eq!(hit.separated, Some(false));
    }

    #[test]
    fn apple_company_alone_is_not_a_tracker() {
        // 0x004C 但 type 0x07（AirPods 之类）：不命中，防误报
        let data = [0x4C, 0x00, 0x07, 0x19, 0x00];
        let adv = ad(0xFF, &data);
        assert!(classify_adv(&adv).is_none());
    }

    #[test]
    fn tile_by_service_data() {
        for uuid in [0xFEEDu16, 0xFEEC, 0xFE84] {
            let data = [ (uuid & 0xFF) as u8, (uuid >> 8) as u8, 0x02, 0x00];
            let adv = ad(0x16, &data);
            let hit = classify_adv(&adv).unwrap();
            assert_eq!(hit.kind, TrackerKind::Tile);
            assert_eq!(hit.separated, None);
        }
    }

    #[test]
    fn smarttag_separated_state() {
        // 首字节 bit5–7 = 2 → 分离
        let data = [0x5A, 0xFD, 0x40];
        let adv = ad(0x16, &data);
        let hit = classify_adv(&adv).unwrap();
        assert_eq!(hit.kind, TrackerKind::SamsungSmartTag);
        assert_eq!(hit.separated, Some(true));
        // bit5–7 = 4 → 配对在网
        let data = [0x5A, 0xFD, 0x80];
        let adv = ad(0x16, &data);
        assert_eq!(classify_adv(&adv).unwrap().separated, Some(false));
    }

    #[test]
    fn samsung_company_id_not_used() {
        // 公司 0x0075 不做判据：所有三星设备都发
        let data = [0x75, 0x00, 0x01, 0x02];
        let adv = ad(0xFF, &data);
        assert!(classify_adv(&adv).is_none());
    }

    #[test]
    fn chipolo_and_fmdn() {
        let data = [0x33, 0xFE, 0x01];
        let adv = ad(0x16, &data);
        assert_eq!(classify_adv(&adv).unwrap().kind, TrackerKind::Chipolo);
        // ONE Spot：公司 0x0157 走 Find My
        let data = [0x57, 0x01, 0x00];
        let adv = ad(0xFF, &data);
        assert_eq!(classify_adv(&adv).unwrap().kind, TrackerKind::Chipolo);
        // FMDN 0x41 = 分离
        let data = [0xAA, 0xFE, 0x41];
        let adv = ad(0x16, &data);
        let hit = classify_adv(&adv).unwrap();
        assert_eq!(hit.kind, TrackerKind::GoogleFmdn);
        assert_eq!(hit.separated, Some(true));
    }

    #[test]
    fn surveillance_names() {
        let adv = ad(0x09, b"Axon Body 2");
        assert_eq!(surveillance_name(&adv), Some("axon body cam"));
        let adv = ad(0x09, b"FREE VENDING");
        assert_eq!(surveillance_name(&adv), Some("possible skimmer"));
        let adv = ad(0x09, b"just a phone");
        assert!(surveillance_name(&adv).is_none());
    }
}
