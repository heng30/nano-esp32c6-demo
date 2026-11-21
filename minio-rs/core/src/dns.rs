//! DNS 劫持应答构造：把任意 A 查询构造成指向网关的 A 记录应答，
//! captive portal 靠它把所有域名解析到板子自己。

/// 应答里 A 记录 TTL（秒）
const ANSWER_TTL: [u8; 4] = [0, 0, 0, 60];

/// 把 DNS 查询 `query` 构造成指向 `ip` 的 A 记录应答，写进 `out` 并返回长度。
/// 只处理未压缩的 QNAME；非 A/IN 查询或包损坏返回 None。
pub fn answer_a(query: &[u8], ip: [u8; 4], out: &mut [u8]) -> Option<usize> {
    // 12 字节头 + 最短问题段（1 字节根标签 + QTYPE/QCLASS 4 字节）
    if query.len() < 17 {
        return None;
    }
    // 跳过 QNAME 标签链找问题段结尾
    let mut pos = 12;
    loop {
        let len = *query.get(pos)?;
        if len & 0xC0 != 0 {
            // 压缩指针只出现在应答里，查询里出现视为损坏
            return None;
        }
        if len == 0 {
            pos += 1;
            break;
        }
        pos += 1 + len as usize;
        if pos + 5 > query.len() {
            return None;
        }
    }
    // QTYPE 必须是 A(1)、QCLASS 必须是 IN(1)
    if query[pos] != 0 || query[pos + 1] != 1 || query[pos + 2] != 0 || query[pos + 3] != 1 {
        return None;
    }
    let qend = pos + 4;
    if out.len() < qend + 16 {
        return None;
    }

    // 应答头：ID 原样、QR 置位、保留查询的 RD、其余清零；ANCount = 1
    out[..12].copy_from_slice(&query[..12]);
    let flags = u16::from_be_bytes([query[2], query[3]]);
    let flags = 0x8000 | (flags & 0x0100);
    out[2..4].copy_from_slice(&flags.to_be_bytes());
    out[4..6].copy_from_slice(&query[4..6]); // QDCount 原样
    out[6..8].copy_from_slice(&1u16.to_be_bytes()); // ANCount
    out[8..10].copy_from_slice(&[0, 0]); // NSCount
    out[10..12].copy_from_slice(&[0, 0]); // ARCount
    // 问题段原样拷贝
    out[12..qend].copy_from_slice(&query[12..qend]);
    // 应答段：指针 0xC00C 指向问题段的 NAME
    out[qend..qend + 2].copy_from_slice(&[0xC0, 0x0C]);
    out[qend + 2..qend + 4].copy_from_slice(&1u16.to_be_bytes()); // TYPE = A
    out[qend + 4..qend + 6].copy_from_slice(&1u16.to_be_bytes()); // CLASS = IN
    out[qend + 6..qend + 10].copy_from_slice(&ANSWER_TTL);
    out[qend + 10..qend + 12].copy_from_slice(&4u16.to_be_bytes()); // RDLENGTH
    out[qend + 12..qend + 16].copy_from_slice(&ip);
    Some(qend + 16)
}

#[cfg(test)]
mod tests {
    // 测试用 Vec，宿主构建下引入 std
    extern crate std;
    use std::vec::Vec;

    use super::*;

    /// 手工构造 example.com 的 A 查询
    fn query_example_com() -> Vec<u8> {
        let mut q = Vec::new();
        q.extend_from_slice(&[0x12, 0x34, 0x01, 0x00]); // ID=1234, RD=1
        q.extend_from_slice(&[0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        q.extend_from_slice(&[7]);
        q.extend_from_slice(b"example");
        q.extend_from_slice(&[3]);
        q.extend_from_slice(b"com");
        q.extend_from_slice(&[0]);
        q.extend_from_slice(&[0, 1, 0, 1]); // A / IN
        q
    }

    #[test]
    fn answer_for_a_query() {
        let q = query_example_com();
        let mut out = [0u8; 128];
        let len = answer_a(&q, [192, 168, 2, 1], &mut out).unwrap();
        let a = &out[..len];
        // ID 保留、QR 置位、RD 保留
        assert_eq!(&a[0..2], &[0x12, 0x34]);
        assert_eq!(u16::from_be_bytes([a[2], a[3]]), 0x8100);
        assert_eq!(u16::from_be_bytes([a[4], a[5]]), 1); // QDCount
        assert_eq!(u16::from_be_bytes([a[6], a[7]]), 1); // ANCount
        // 问题段原样保留
        assert_eq!(&a[12..29], &q[12..29]);
        // 指针 + A + IN + rdlen 4 + IP
        assert_eq!(&a[29..31], &[0xC0, 0x0C]);
        assert_eq!(u16::from_be_bytes([a[31], a[32]]), 1);
        assert_eq!(u16::from_be_bytes([a[33], a[34]]), 1);
        assert_eq!(u16::from_be_bytes([a[39], a[40]]), 4);
        assert_eq!(&a[41..45], &[192, 168, 2, 1]);
        assert_eq!(len, 45);
    }

    #[test]
    fn non_a_query_rejected() {
        let mut q = query_example_com();
        // QTYPE 改成 AAAA (28)
        let qlen = q.len();
        q[qlen - 4] = 0;
        q[qlen - 3] = 28;
        let mut out = [0u8; 128];
        assert!(answer_a(&q, [1, 2, 3, 4], &mut out).is_none());
    }

    #[test]
    fn truncated_rejected() {
        let q = query_example_com();
        let mut out = [0u8; 128];
        assert!(answer_a(&q[..12], [1, 2, 3, 4], &mut out).is_none());
        assert!(answer_a(&q[..20], [1, 2, 3, 4], &mut out).is_none());
    }

    #[test]
    fn compressed_qname_rejected() {
        let mut q = query_example_com();
        // 把 QNAME 首字节换成压缩指针形态
        q[12] = 0xC0;
        let mut out = [0u8; 128];
        assert!(answer_a(&q, [1, 2, 3, 4], &mut out).is_none());
    }
}
