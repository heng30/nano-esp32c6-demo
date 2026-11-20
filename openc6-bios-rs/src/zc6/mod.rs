//! ZC6 v4 压缩格式：常量、文件头与 CRC32。

pub mod compress;
pub mod decompress;

pub use compress::compress;
pub use decompress::decompress;

/// 魔数 "!ZC6"（小端字节序 `21 5A 43 36`）。
pub const MAGIC: u32 = 0x3643_5A21;
/// 格式版本。
pub const VERSION: u16 = 4;
/// 文件头字节数。
pub const HEADER_SIZE: usize = 20;

/// 滑动窗口上限，与 4 KB 页分配器对齐。
pub const MAX_WINDOW_SIZE: usize = 4096;
/// TinyMatch 的最大回溯距离。
pub const TINY_DIST_MAX: usize = 16;
/// TinyMatch 的最短匹配长度。
pub const TINY_LEN_MIN: usize = 3;
/// TinyMatch 的最长匹配长度。
pub const TINY_LEN_MAX: usize = 6;

/// ShortMatch 的最大回溯距离。
pub const SHORT_DIST_MAX: usize = 255;
/// 所有 LZ 匹配的最短长度。
pub const MIN_MATCH_LEN: usize = 3;
/// 所有 LZ 匹配的最长长度。
pub const MAX_MATCH_LEN: usize = 66;

/// 主命令标签占高 2 位。
const OP_MASK: u8 = 0xC0;
/// 1 字节 token：距离 ≤ 16，长度 3..6。
const OP_TINY_MATCH: u8 = 0x00;
/// 2 字节 token：距离 ≤ 255，长度 3..66。
const OP_SHORT_MATCH: u8 = 0x40;
/// 3 字节 token：距离 > 255，长度 3..66。
const OP_LONG_MATCH: u8 = 0x80;
/// 特殊模式：字面量 / 零串 / RLE / 稀疏字。
const OP_SPECIAL: u8 = 0xC0;
/// 低 6 位是命令参数。
const PAYLOAD_MASK: u8 = 0x3F;

/// 单个字面量块的最大字节数。
const SPECIAL_LITERAL_MAX: usize = 32;
/// 多字零串子模式。
const SPECIAL_ZERO_RUN: u8 = 0x20;
/// 长字节 RLE 子模式。
const SPECIAL_RLE_LONG: u8 = 0x21;
/// 稀疏 32 位字子模式起始值。
const SPECIAL_SPARSE_BASE: u8 = 0x22;
/// 短字节 RLE 子模式起始值（长度 4..19）。
const SPECIAL_RLE_SHORT: u8 = 0x30;

/// ZC6 归档头（20 字节，小端）。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, defmt::Format)]
pub struct Header {
    /// 必须等于 [`MAGIC`]。
    pub magic: u32,
    /// 格式版本，必须是 [`VERSION`]。
    pub version: u16,
    /// 保留特性位。
    pub flags: u16,
    /// 解压后的字节数。
    pub raw_size: u32,
    /// 压缩负载字节数（不含头）。
    pub comp_size: u32,
    /// 原始数据的 CRC32。
    pub checksum: u32,
}

impl Header {
    /// 从字节流解析头部，魔数或版本不符时返回 `None`。
    pub fn parse(src: &[u8]) -> Option<Self> {
        if src.len() < HEADER_SIZE {
            return None;
        }
        let hdr = unsafe { core::ptr::read_unaligned(src.as_ptr() as *const Self) };
        (hdr.magic == MAGIC && hdr.version == VERSION).then_some(hdr)
    }

    /// 序列化到 20 字节。
    pub fn to_bytes(self) -> [u8; HEADER_SIZE] {
        let mut out = [0u8; HEADER_SIZE];
        unsafe {
            core::ptr::write_unaligned(out.as_mut_ptr() as *mut Self, self);
        }
        out
    }
}

/// 标准 IEEE 802.3 CRC32。
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

/// 压缩 → 解压回环自检，POST 时无条件跑一次，验证编解码器一致。
///
/// 覆盖字面量、零串、字节 RLE 与重复串四种路径，512 字节量级，耗时可忽略。
pub fn self_check() -> bool {
    // 混合模式：重复串（LZ）、零串、RLE、以及不可压缩的字面量
    let mut src = [0u8; 512];
    for (i, b) in src.iter_mut().enumerate() {
        *b = match i {
            0..64 => (i % 7) as u8,
            64..192 => 0,
            192..256 => 0xAA,
            _ => (i * 31 % 251) as u8,
        };
    }

    let mut comp = [0u8; 512 + HEADER_SIZE + 128];
    let Some(comp_len) = compress(&src, &mut comp) else {
        return false;
    };

    let mut back = [0u8; 512];
    decompress(&comp[..comp_len], &mut back) == Some(src.len()) && back == src
}
