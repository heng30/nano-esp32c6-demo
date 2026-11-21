//! IEEE 802.15.4 MAC 帧头解析：FCF/序号/地址/命令 id，Zigbee/Thread 嗅探共用。
//! 输入是 PSDU（PHY 长度字节已剥掉），尾部 FCS 不影响头解析。
//! 只覆盖 2003/2006 版（frame version 0/1）：2015 版的 IE 与序号抑制不支持。

/// Beacon Request 的 MAC 命令 id
pub const CMD_BEACON_REQUEST: u8 = 0x09;

/// 帧类型（FCF bit 0–2）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType154 {
    Beacon,
    Data,
    Ack,
    Command,
}

impl FrameType154 {
    /// 日志用短名
    pub fn name(self) -> &'static str {
        match self {
            Self::Beacon => "beacon",
            Self::Data => "data",
            Self::Ack => "ack",
            Self::Command => "cmd",
        }
    }
}

/// 地址字段形态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addr154 {
    /// 无地址
    None,
    /// 16 位短地址
    Short(u16),
    /// 64 位扩展地址，低位在前
    Ext([u8; 8]),
}

/// 解析结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame154 {
    /// 帧类型
    pub ftype: FrameType154,
    /// 帧序号
    pub seq: u8,
    /// 目的 PAN id
    pub dst_pan: Option<u16>,
    /// 目的地址
    pub dst: Addr154,
    /// 源 PAN id（PAN 压缩时等于目的 PAN）
    pub src_pan: Option<u16>,
    /// 源地址
    pub src: Addr154,
    /// MAC 头字节数；其后是负载（Command 帧首字节为命令 id）
    pub header_len: usize,
    /// Command 帧的命令 id
    pub command: Option<u8>,
}

/// 是否 Beacon Request（Zigbee 扫描最常见的命令帧）
pub fn is_beacon_request(f: &Frame154) -> bool {
    matches!(f.ftype, FrameType154::Command) && f.command == Some(CMD_BEACON_REQUEST)
}

/// 读一个地址字段，返回（地址，新偏移）
fn read_addr(buf: &[u8], pos: usize, mode: u16) -> Option<(Addr154, usize)> {
    match mode {
        1 => {
            let a = u16::from_le_bytes(buf.get(pos..pos + 2)?.try_into().ok()?);
            Some((Addr154::Short(a), pos + 2))
        }
        2 => {
            let b: [u8; 8] = buf.get(pos..pos + 8)?.try_into().ok()?;
            Some((Addr154::Ext(b), pos + 8))
        }
        // 3 = 保留值
        _ => None,
    }
}

/// 解析 PSDU；格式不支持或被截断返回 None
pub fn parse(buf: &[u8]) -> Option<Frame154> {
    if buf.len() < 3 {
        return None;
    }
    let fcf = u16::from_le_bytes([buf[0], buf[1]]);
    let ftype = match fcf & 0x7 {
        0 => FrameType154::Beacon,
        1 => FrameType154::Data,
        2 => FrameType154::Ack,
        3 => FrameType154::Command,
        _ => return None,
    };
    // 安全帧（辅助安全头）不支持
    if fcf & 0x8 != 0 {
        return None;
    }
    // 2015 版（version 2）带 IE 与序号抑制，不支持
    if (fcf >> 12) & 0x3 == 2 {
        return None;
    }
    let seq = buf[2];
    let dst_mode = (fcf >> 10) & 0x3;
    let src_mode = (fcf >> 14) & 0x3;
    let panid_comp = fcf & 0x40 != 0;

    let mut pos = 3usize;
    let mut dst_pan = None;
    let mut dst = Addr154::None;
    if dst_mode != 0 {
        dst_pan = Some(u16::from_le_bytes(buf.get(pos..pos + 2)?.try_into().ok()?));
        pos += 2;
        let (d, p) = read_addr(buf, pos, dst_mode)?;
        dst = d;
        pos = p;
    }
    let mut src_pan = None;
    let mut src = Addr154::None;
    if src_mode != 0 {
        // PAN 压缩仅在存在目的地址时有效，否则仍要读源 PAN
        if !panid_comp || dst_mode == 0 {
            src_pan = Some(u16::from_le_bytes(buf.get(pos..pos + 2)?.try_into().ok()?));
            pos += 2;
        } else {
            src_pan = dst_pan;
        }
        let (s, p) = read_addr(buf, pos, src_mode)?;
        src = s;
        pos = p;
    }
    let command = if matches!(ftype, FrameType154::Command) {
        Some(*buf.get(pos)?)
    } else {
        None
    };
    Some(Frame154 {
        ftype,
        seq,
        dst_pan,
        dst,
        src_pan,
        src,
        header_len: pos,
        command,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 标准广播 Beacon Request：命令帧 + 目的短地址广播
    #[test]
    fn beacon_request() {
        let buf = [0x03, 0x04, 0x2A, 0xFF, 0xFF, 0xFF, 0xFF, 0x09];
        let f = parse(&buf).unwrap();
        assert_eq!(f.ftype, FrameType154::Command);
        assert_eq!(f.seq, 0x2A);
        assert_eq!(f.dst_pan, Some(0xFFFF));
        assert_eq!(f.dst, Addr154::Short(0xFFFF));
        assert_eq!(f.src, Addr154::None);
        assert_eq!(f.header_len, 7);
        assert_eq!(f.command, Some(CMD_BEACON_REQUEST));
        assert!(is_beacon_request(&f));
    }

    /// 带源短地址 + PAN 压缩的数据帧
    #[test]
    fn data_frame_with_panid_compression() {
        // FCF = data(1) | dst short(0x0400) | panid comp(0x40) | src short(0x4000) = 0x4441
        let buf = [0x41, 0x44, 0x10, 0x34, 0x12, 0xAA, 0xBB, 0x01, 0x02, 0xDE, 0xAD];
        let f = parse(&buf).unwrap();
        assert_eq!(f.ftype, FrameType154::Data);
        assert_eq!(f.dst_pan, Some(0x1234));
        assert_eq!(f.dst, Addr154::Short(0xBBAA));
        assert_eq!(f.src_pan, Some(0x1234)); // 压缩：等于目的 PAN
        assert_eq!(f.src, Addr154::Short(0x0201));
        assert_eq!(f.header_len, 9);
        assert_eq!(f.command, None);
    }

    /// 3 字节裸 ACK 帧
    #[test]
    fn ack_frame() {
        let f = parse(&[0x02, 0x00, 0x33]).unwrap();
        assert_eq!(f.ftype, FrameType154::Ack);
        assert_eq!(f.seq, 0x33);
        assert_eq!(f.header_len, 3);
        assert_eq!(f.dst, Addr154::None);
    }

    /// 源 64 位扩展地址（无目的地址）
    #[test]
    fn ext_src_addr() {
        // FCF = beacon(0) | src ext(0x8000) = 0x8000
        let buf = [
            0x00, 0x80, 0x01, 0x34, 0x12, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
        ];
        let f = parse(&buf).unwrap();
        assert_eq!(f.ftype, FrameType154::Beacon);
        // 无目的地址且未压缩：源 PAN 要读
        assert_eq!(f.src_pan, Some(0x1234));
        assert_eq!(f.src, Addr154::Ext([0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]));
        assert_eq!(f.header_len, 13);
    }

    #[test]
    fn truncated_rejected() {
        assert!(parse(&[0x03]).is_none());
        assert!(parse(&[0x00; 2]).is_none());
        // 头还没读完就断了
        let cut = [0x03, 0x04, 0x00, 0xFF];
        assert!(parse(&cut).is_none());
    }

    #[test]
    fn security_and_2015_rejected() {
        // 安全位（0x08）
        assert!(parse(&[0x0B, 0x00, 0x01]).is_none());
        // frame version 2（0x2000）
        assert!(parse(&[0x00, 0x20, 0x01]).is_none());
    }
}
