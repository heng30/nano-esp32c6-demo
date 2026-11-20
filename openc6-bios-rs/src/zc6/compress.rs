//! ZC6 v4 压缩器：1 字节 TinyMatch + 短/长匹配 + RLE + 零串 + 稀疏字。

use alloc::boxed::Box;

use super::*;

/// 匹配查找器的哈希槽数量。嵌入式上用 256 个槽，约 1.3 KB。
const HASH_SIZE: usize = 256;
/// 每个哈希槽的候选路数。
const HASH_WAYS: usize = 2;
/// 哈希槽掩码。
const HASH_MASK: u32 = HASH_SIZE as u32 - 1;

/// 哈希表驱动的匹配查找器。
struct MatchFinder {
    /// 每个槽保存的候选位置 +1（0 表示空）。
    entries: [[u16; HASH_WAYS]; HASH_SIZE],
    /// 每个槽下一次写入的路径索引。
    head: [u8; HASH_SIZE],
}

impl MatchFinder {
    /// 全零初始化的常量，用来把大数组直接搬到堆上而不占栈。
    const ZEROED: Self = Self {
        entries: [[0; HASH_WAYS]; HASH_SIZE],
        head: [0; HASH_SIZE],
    };

    fn new() -> Box<Self> {
        Box::new(Self::ZEROED)
    }

    /// 把 `pos` 登记进哈希表。
    fn insert(&mut self, src: &[u8], pos: usize) {
        if pos + MIN_MATCH_LEN <= src.len() {
            let h = hash(src, pos);
            let slot = self.head[h] as usize;
            self.entries[h][slot] = (pos + 1) as u16;
            self.head[h] = ((slot + 1) % HASH_WAYS) as u8;
        }
    }

    /// 在窗口内找最长匹配，返回 `(长度, 距离)`；找不到返回 `(0, 0)`。
    fn best_match(&self, src: &[u8], pos: usize) -> (usize, usize) {
        if pos + MIN_MATCH_LEN > src.len() {
            return (0, 0);
        }

        let h = hash(src, pos);
        let mut best_len = 0;
        let mut best_dist = 0;

        for way in 0..HASH_WAYS {
            let cand = self.entries[h][way] as usize;
            if cand == 0 {
                continue;
            }
            let cand_idx = cand - 1;
            let dist = pos.wrapping_sub(cand_idx);
            if dist == 0 || dist > MAX_WINDOW_SIZE {
                continue;
            }

            let mut mlen = 0;
            while pos + mlen < src.len()
                && mlen < MAX_MATCH_LEN
                && src[pos + mlen] == src[cand_idx + mlen]
            {
                mlen += 1;
            }
            if mlen > best_len {
                best_len = mlen;
                best_dist = dist;
                if best_len == MAX_MATCH_LEN {
                    break;
                }
            }
        }

        (best_len, best_dist)
    }
}

fn hash(src: &[u8], pos: usize) -> usize {
    (((src[pos] as u32) << 8) ^ ((src[pos + 1] as u32) << 4) ^ src[pos + 2] as u32) as usize
        & HASH_MASK as usize
}

/// 把累积的原始字面量刷进输出流，返回写入的字节数。
fn flush_literals(dst: &mut [u8], di: usize, lits: &[u8]) -> Option<usize> {
    let mut written = 0;
    let mut offset = 0;

    while offset < lits.len() {
        let chunk = (lits.len() - offset).min(SPECIAL_LITERAL_MAX);
        if di + written + 1 + chunk > dst.len() {
            return None;
        }
        dst[di + written] = OP_SPECIAL | (chunk as u8 - 1);
        written += 1;
        dst[di + written..di + written + chunk].copy_from_slice(&lits[offset..offset + chunk]);
        written += chunk;
        offset += chunk;
    }

    Some(written)
}

/// 压缩 `src` 到 `dst`，成功返回写入的总字节数（含 20 字节头）。
///
/// `dst` 必须至少 `src.len() + HEADER_SIZE + 128` 字节，与上游要求一致。
pub fn compress(src: &[u8], dst: &mut [u8]) -> Option<usize> {
    if src.is_empty() || dst.len() < HEADER_SIZE + src.len() + 128 {
        return None;
    }

    let dst_cap = dst.len();
    let mut mf = MatchFinder::new();
    let mut di = HEADER_SIZE;

    let mut si = 0usize;
    let mut lit_start = 0usize;
    let mut lit_count = 0usize;

    while si < src.len() {
        // 1. 字节级 RLE
        let rle_val = src[si];
        let mut rle_len = 0usize;
        while si + rle_len < src.len() && src[si + rle_len] == rle_val && rle_len < 255 {
            rle_len += 1;
        }
        let rle_savings = if rle_len >= 4 {
            if rle_len <= 19 {
                rle_len as i32 - 2
            } else {
                rle_len as i32 - 3
            }
        } else {
            0
        };

        // 2. 多字零串
        let mut zero_words = 0usize;
        if si + 4 <= src.len() && src[si..si + 4] == [0, 0, 0, 0] {
            let mut probe = si;
            while probe + 4 <= src.len() && zero_words < 255 && src[probe..probe + 4] == [0, 0, 0, 0]
            {
                zero_words += 1;
                probe += 4;
            }
        }
        let zero_savings = if zero_words > 0 {
            zero_words as i32 * 4 - 2
        } else {
            0
        };

        // 3. LZ 匹配仲裁
        let (mut best_len, best_dist) = mf.best_match(src, si);
        let mut lz_savings = 0i32;
        let mut use_tiny = false;

        if best_dist <= TINY_DIST_MAX && best_len >= TINY_LEN_MIN {
            let tlen = best_len.min(TINY_LEN_MAX);
            let tiny_sav = tlen as i32 - 1;
            let short_sav = best_len as i32 - 2;
            if short_sav > tiny_sav {
                lz_savings = short_sav;
                use_tiny = false;
            } else {
                lz_savings = tiny_sav;
                best_len = tlen;
                use_tiny = true;
            }
        } else if best_dist <= SHORT_DIST_MAX && best_len >= MIN_MATCH_LEN {
            lz_savings = best_len as i32 - 2;
        } else if best_dist > SHORT_DIST_MAX && best_len >= 4 {
            lz_savings = best_len as i32 - 3;
        } else {
            best_len = 0;
        }

        // 惰性匹配：看一眼下一个位置是否更划算
        if lz_savings > 0 && best_len < MAX_MATCH_LEN && si + 1 < src.len() {
            let (next_len, next_dist) = mf.best_match(src, si + 1);
            let next_sav = if next_dist <= TINY_DIST_MAX && next_len >= TINY_LEN_MIN {
                let ntlen = next_len.min(TINY_LEN_MAX);
                (next_len as i32 - 2).max(ntlen as i32 - 1)
            } else if next_dist <= SHORT_DIST_MAX && next_len >= MIN_MATCH_LEN {
                next_len as i32 - 2
            } else if next_dist > SHORT_DIST_MAX && next_len >= 4 {
                next_len as i32 - 3
            } else {
                0
            };

            if next_sav > lz_savings + 1 {
                mf.insert(src, si);
                lit_count += 1;
                si += 1;
                continue;
            }
        }

        mf.insert(src, si);

        // 4. 稀疏 32 位字
        let mut sparse_mask = 0u8;
        let mut non_zero_count = 4i32;
        if si + 4 <= src.len() && zero_words == 0 {
            let b = &src[si..si + 4];
            sparse_mask = (b[0] != 0) as u8
                | ((b[1] != 0) as u8) << 1
                | ((b[2] != 0) as u8) << 2
                | ((b[3] != 0) as u8) << 3;
            non_zero_count = b.iter().filter(|&&x| x != 0).count() as i32;
        }
        let sparse_savings = if non_zero_count <= 2 && sparse_mask > 0 && sparse_mask < 15 {
            4 - (1 + non_zero_count)
        } else {
            0
        };

        // 5. 收益仲裁：RLE > 零串 > LZ > 稀疏 > 字面量
        if rle_savings > 0 && rle_savings >= lz_savings && rle_savings >= zero_savings {
            flush(&mut dst[..dst_cap], &mut di, &src[lit_start..lit_start + lit_count])?;
            lit_count = 0;

            if rle_len <= 19 {
                dst[di] = OP_SPECIAL | (SPECIAL_RLE_SHORT + (rle_len as u8 - 4));
                dst[di + 1] = rle_val;
                di += 2;
            } else {
                dst[di] = OP_SPECIAL | SPECIAL_RLE_LONG;
                dst[di + 1] = rle_len as u8;
                dst[di + 2] = rle_val;
                di += 3;
            }

            for k in 1..rle_len {
                mf.insert(src, si + k);
            }
            si += rle_len;
            lit_start = si;
            continue;
        }

        if zero_savings > 0 && zero_savings >= lz_savings {
            flush(&mut dst[..dst_cap], &mut di, &src[lit_start..lit_start + lit_count])?;
            lit_count = 0;

            dst[di] = OP_SPECIAL | SPECIAL_ZERO_RUN;
            dst[di + 1] = zero_words as u8;
            di += 2;

            for k in 1..zero_words * 4 {
                mf.insert(src, si + k);
            }
            si += zero_words * 4;
            lit_start = si;
            continue;
        }

        if lz_savings > 0 && lz_savings >= sparse_savings {
            flush(&mut dst[..dst_cap], &mut di, &src[lit_start..lit_start + lit_count])?;
            lit_count = 0;

            if use_tiny {
                let len_bits = ((best_len - TINY_LEN_MIN) & 0x03) as u8;
                let dist_bits = ((best_dist - 1) & 0x0F) as u8;
                dst[di] = OP_TINY_MATCH | (len_bits << 4) | dist_bits;
                di += 1;
            } else if best_dist <= SHORT_DIST_MAX {
                dst[di] = OP_SHORT_MATCH | (best_len as u8 - MIN_MATCH_LEN as u8);
                dst[di + 1] = best_dist as u8;
                di += 2;
            } else {
                dst[di] = OP_LONG_MATCH | (best_len as u8 - MIN_MATCH_LEN as u8);
                dst[di + 1] = (best_dist & 0xFF) as u8;
                dst[di + 2] = ((best_dist >> 8) & 0xFF) as u8;
                di += 3;
            }

            for k in 1..best_len {
                mf.insert(src, si + k);
            }
            si += best_len;
            lit_start = si;
            continue;
        }

        if sparse_savings > 0 {
            flush(&mut dst[..dst_cap], &mut di, &src[lit_start..lit_start + lit_count])?;
            lit_count = 0;

            dst[di] = OP_SPECIAL | (SPECIAL_SPARSE_BASE + (sparse_mask - 1));
            di += 1;
            for (bit, &byte) in src[si..si + 4].iter().enumerate() {
                if sparse_mask & (1 << bit) != 0 {
                    dst[di] = byte;
                    di += 1;
                }
            }

            for k in 1..4 {
                mf.insert(src, si + k);
            }
            si += 4;
            lit_start = si;
            continue;
        }

        // 6. 攒字面量
        lit_count += 1;
        si += 1;
    }

    flush(&mut dst[..dst_cap], &mut di, &src[lit_start..lit_start + lit_count])?;

    let hdr = Header {
        magic: MAGIC,
        version: VERSION,
        flags: 0,
        raw_size: src.len() as u32,
        comp_size: (di - HEADER_SIZE) as u32,
        checksum: crc32(src),
    };
    dst[..HEADER_SIZE].copy_from_slice(&hdr.to_bytes());

    Some(di)
}

/// 刷字面量并推进写指针，空块直接跳过。
fn flush(dst: &mut [u8], di: &mut usize, lits: &[u8]) -> Option<()> {
    if lits.is_empty() {
        return Some(());
    }
    let n = flush_literals(dst, *di, lits)?;
    *di += n;
    Some(())
}
