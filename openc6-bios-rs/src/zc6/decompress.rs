//! ZC6 v4 流式解压器，零动态内存。

use super::*;

/// 解压 `src` 到 `dst`，成功时返回解压后的字节数。
///
/// 会校验魔数、版本、长度一致性和 CRC32；任何一步不符都返回 `None`。
pub fn decompress(src: &[u8], dst: &mut [u8]) -> Option<usize> {
    let hdr = Header::parse(src)?;

    if hdr.raw_size as usize > dst.len() {
        return None;
    }

    let comp_end = HEADER_SIZE + hdr.comp_size as usize;
    if comp_end > src.len() {
        return None;
    }

    let mut si = HEADER_SIZE;
    let mut di = 0usize;
    let raw_size = hdr.raw_size as usize;

    while si < comp_end && di < raw_size {
        let tag = src[si];
        si += 1;
        let opcode = tag & OP_MASK;
        let param = (tag & PAYLOAD_MASK) as usize;

        match opcode {
            OP_TINY_MATCH => {
                let len = ((param >> 4) & 0x03) + TINY_LEN_MIN;
                let dist = (param & 0x0F) + 1;
                if dist > di || di + len > dst.len() {
                    return None;
                }
                copy_window(dst, di, dist, len);
                di += len;
            }
            OP_SHORT_MATCH => {
                let len = param + MIN_MATCH_LEN;
                if si >= comp_end {
                    return None;
                }
                let dist = src[si] as usize;
                si += 1;
                if dist == 0 || dist > di || di + len > dst.len() {
                    return None;
                }
                copy_window(dst, di, dist, len);
                di += len;
            }
            OP_LONG_MATCH => {
                let len = param + MIN_MATCH_LEN;
                if si + 2 > comp_end {
                    return None;
                }
                let dist = src[si] as usize | (src[si + 1] as usize) << 8;
                si += 2;
                if dist == 0 || dist > di || di + len > dst.len() {
                    return None;
                }
                copy_window(dst, di, dist, len);
                di += len;
            }
            OP_SPECIAL => {
                if param <= 31 {
                    // 原始字面量 1..32 字节
                    let count = param + 1;
                    if si + count > comp_end || di + count > dst.len() {
                        return None;
                    }
                    dst[di..di + count].copy_from_slice(&src[si..si + count]);
                    si += count;
                    di += count;
                } else if param == SPECIAL_ZERO_RUN as usize {
                    // 多字零串 1..255 个字
                    if si >= comp_end {
                        return None;
                    }
                    let zero_bytes = src[si] as usize * 4;
                    si += 1;
                    if di + zero_bytes > dst.len() {
                        return None;
                    }
                    dst[di..di + zero_bytes].fill(0);
                    di += zero_bytes;
                } else if param == SPECIAL_RLE_LONG as usize {
                    // 长字节 RLE 1..255 字节
                    if si + 2 > comp_end {
                        return None;
                    }
                    let run_len = src[si] as usize;
                    let val = src[si + 1];
                    si += 2;
                    if di + run_len > dst.len() {
                        return None;
                    }
                    dst[di..di + run_len].fill(val);
                    di += run_len;
                } else if param >= SPECIAL_SPARSE_BASE as usize
                    && param < SPECIAL_RLE_SHORT as usize
                {
                    // 稀疏 32 位字，掩码 1..14
                    let mask = (param - SPECIAL_SPARSE_BASE as usize) + 1;
                    let mut word = 0u32;
                    for bit in 0..4 {
                        if mask & (1 << bit) != 0 {
                            if si >= comp_end {
                                return None;
                            }
                            word |= (src[si] as u32) << (bit * 8);
                            si += 1;
                        }
                    }
                    if di + 4 > dst.len() {
                        return None;
                    }
                    dst[di..di + 4].copy_from_slice(&word.to_le_bytes());
                    di += 4;
                } else if param >= SPECIAL_RLE_SHORT as usize {
                    // 短字节 RLE 4..19 字节
                    let run_len = (param - SPECIAL_RLE_SHORT as usize) + 4;
                    if si >= comp_end {
                        return None;
                    }
                    let val = src[si];
                    si += 1;
                    if di + run_len > dst.len() {
                        return None;
                    }
                    dst[di..di + run_len].fill(val);
                    di += run_len;
                } else {
                    return None;
                }
            }
            _ => return None,
        }
    }

    if di != raw_size || crc32(&dst[..di]) != hdr.checksum {
        return None;
    }

    Some(di)
}

/// 从 `dst[di - dist..]` 逐字节复制 `len` 字节到 `dst[di..]`，允许窗口重叠。
fn copy_window(dst: &mut [u8], di: usize, dist: usize, len: usize) {
    for i in 0..len {
        dst[di + i] = dst[di + i - dist];
    }
}
