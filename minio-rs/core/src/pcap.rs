//! pcap 文件格式写入器：全局头 + 包记录帧封装，对应上游各嗅探组件的落盘格式。
//! 产物可直接被 Wireshark 打开。

/// 链路类型，落在 pcap 全局头 network 字段
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkType {
    /// 原始 802.11 帧（DLT_IEEE802_11）
    Ieee80211 = 105,
    /// 802.15.4 含 FCS（DLT_IEEE802_15_4_WITHFCS），Zigbee/Thread 嗅探用
    Ieee802154WithFcs = 195,
    /// 蓝牙 HCI H4（DLT_BLUETOOTH_HCI_H4_WITH_PHDR）
    BleHci = 201,
}

/// pcap 全局头长度
pub const GLOBAL_HEADER_LEN: usize = 24;
/// pcap 包记录头长度
pub const PACKET_HEADER_LEN: usize = 16;

/// 经典 pcap 魔数（小端序 0xa1b2c3d4）
const MAGIC: u32 = 0xA1B2_C3D4;

/// 把全局头写进 `out`；文件头版本 2.4、snaplen 65535
pub fn write_global_header(out: &mut [u8; GLOBAL_HEADER_LEN], link: LinkType) {
    out.fill(0);
    out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    out[4..6].copy_from_slice(&2u16.to_le_bytes()); // version major
    out[6..8].copy_from_slice(&4u16.to_le_bytes()); // version minor
    // 8..12 thiszone = 0、12..16 sigfigs = 0 已由 fill(0) 保证
    out[16..20].copy_from_slice(&65535u32.to_le_bytes()); // snaplen
    out[20..24].copy_from_slice(&(link as u32).to_le_bytes());
}

/// 把包记录头写进 `out`；`len` 为该包实际字节数（含链路层填充）
pub fn write_packet_header(out: &mut [u8; PACKET_HEADER_LEN], ts_secs: u32, ts_usecs: u32, len: u32) {
    out[0..4].copy_from_slice(&ts_secs.to_le_bytes());
    out[4..8].copy_from_slice(&ts_usecs.to_le_bytes());
    out[8..12].copy_from_slice(&len.to_le_bytes()); // incl_len
    out[12..16].copy_from_slice(&len.to_le_bytes()); // orig_len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_header_matches_reference_bytes() {
        let mut buf = [0u8; GLOBAL_HEADER_LEN];
        write_global_header(&mut buf, LinkType::Ieee802154WithFcs);
        assert_eq!(&buf[0..4], &[0xD4, 0xC3, 0xB2, 0xA1]); // 魔数小端
        assert_eq!(u16::from_le_bytes([buf[4], buf[5]]), 2);
        assert_eq!(u16::from_le_bytes([buf[6], buf[7]]), 4);
        assert_eq!(&buf[8..16], &[0u8; 8]); // thiszone + sigfigs
        assert_eq!(u32::from_le_bytes(buf[16..20].try_into().unwrap()), 65535);
        assert_eq!(u32::from_le_bytes(buf[20..24].try_into().unwrap()), 195);
    }

    #[test]
    fn link_type_values() {
        // 对照 Wireshark DLT 头文件常量，写错了 Wireshark 解不开
        assert_eq!(LinkType::Ieee80211 as u32, 105);
        assert_eq!(LinkType::Ieee802154WithFcs as u32, 195);
        assert_eq!(LinkType::BleHci as u32, 201);
    }

    #[test]
    fn packet_header_framing() {
        let mut buf = [0u8; PACKET_HEADER_LEN];
        write_packet_header(&mut buf, 1_700_000_000, 250_000, 127);
        assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), 1_700_000_000);
        assert_eq!(u32::from_le_bytes(buf[4..8].try_into().unwrap()), 250_000);
        assert_eq!(u32::from_le_bytes(buf[8..12].try_into().unwrap()), 127);
        assert_eq!(u32::from_le_bytes(buf[12..16].try_into().unwrap()), 127);
    }
}
