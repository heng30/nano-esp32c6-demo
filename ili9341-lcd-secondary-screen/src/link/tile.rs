//! 一块 tile 的解析与解码：布局、四种编码、铺回 16×16 写屏。

use core::sync::atomic::Ordering;

use defmt::warn;
use embassy_futures::yield_now;
use esp_hal::time::Instant;

use super::pool::SCALE;
use super::protocol::{KIND_PAL2, KIND_PAL4, KIND_RAW, KIND_RLE, MAX_TILES, TILE, TILE_HEAD};
use crate::display::Display;

/// 一块数据的哈希（FNV-1a）：上位机按同样的算法核对板子收到的是不是同一份字节。
fn tile_hash(bytes: &[u8]) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// 一批里每块 tile 的 `(头偏移, 数据偏移, 数据长度)`；布局不合法返回 `None`。
fn tile_spans(
    tiles: &[u8],
    count: usize,
) -> Option<impl Iterator<Item = (usize, usize, usize)> + '_> {
    let mut spans = [0usize; MAX_TILES * 3];
    let mut at = 0;
    for index in 0..count {
        let head = at;
        if head + TILE_HEAD > tiles.len() {
            return None;
        }
        let len = usize::from(u16::from_le_bytes([tiles[head + 3], tiles[head + 4]]));
        let body = head + TILE_HEAD;
        if body + len > tiles.len() {
            return None;
        }
        spans[index * 3] = head;
        spans[index * 3 + 1] = body;
        spans[index * 3 + 2] = len;
        at = body + len;
    }
    (at == tiles.len()).then(|| {
        (0..count).map(move |index| {
            let head = spans[index * 3];
            (head, spans[index * 3 + 1], spans[index * 3 + 2])
        })
    })
}

/// 解一块游程编码的 tile；目标像素数由调用方按当前放大倍数给出。
fn decode_rle(data: &[u8], pixels: &mut [u16]) -> bool {
    let mut at = 0;
    let mut index = 0;
    while at < data.len() {
        let tag = data[at];
        at += 1;
        let count = usize::from(tag & 0x7F) + 1;
        if index + count > pixels.len() {
            return false;
        }
        if tag & 0x80 != 0 {
            if at + 2 > data.len() {
                return false;
            }
            let pixel = u16::from_le_bytes([data[at], data[at + 1]]);
            at += 2;
            for dst in &mut pixels[index..index + count] {
                *dst = pixel;
            }
        } else {
            if at + count * 2 > data.len() {
                return false;
            }
            for dst in &mut pixels[index..index + count] {
                *dst = u16::from_le_bytes([data[at], data[at + 1]]);
                at += 2;
            }
        }
        index += count;
    }
    index == pixels.len()
}

/// 解一块调色板编码的 tile：`n` 个 u16 颜色 + 打包索引（little-endian 位序）。
fn decode_palette(data: &[u8], pixels: &mut [u16], bits: u8) -> bool {
    let colors = 1usize << bits;
    let per_byte = 8 / bits as usize;
    let packed_len = pixels.len().div_ceil(per_byte);
    if data.len() != colors * 2 + packed_len {
        return false;
    }
    let (table, packed) = data.split_at(colors * 2);
    let mask = (1u8 << bits) - 1;
    for (index, &byte) in packed.iter().enumerate() {
        for slot in 0..per_byte {
            let at = index * per_byte + slot;
            if at >= pixels.len() {
                break;
            }
            let code = usize::from((byte >> (slot * bits as usize)) & mask);
            let entry = code * 2;
            pixels[at] = u16::from_le_bytes([table[entry], table[entry + 1]]);
        }
    }
    true
}

/// 画一批 tile，返回 `(画好的块数, 这批字节的哈希之和, 纯绘制耗时微秒)`。
/// 每块里装的是 `(16/scale)²` 个源像素，这里按 scale 铺成 16×16 再整块送出。
pub(super) async fn draw_tiles(
    display: &mut Display,
    tiles: &[u8],
    count: usize,
) -> (u16, u32, u64) {
    let Some(spans) = tile_spans(tiles, count) else {
        warn!("bad tile layout in batch of {}", count);
        return (0, 0, 0);
    };

    // 采样倍数决定每块里的源像素边长；16 能被它整除，拼回来正好 TILE
    let scale = usize::from(SCALE.load(Ordering::Relaxed)).clamp(1, TILE);
    let arm = TILE / scale;
    let mut source = [0u16; TILE * TILE];
    let mut pixels = [0u16; TILE * TILE];
    let mut drawn = 0u16;
    let mut hash = 0u32;
    let mut draw_us = 0u64;
    for (head, body, len) in spans {
        let data = &tiles[body..body + len];
        match tiles[head + 2] {
            KIND_RLE => {
                if !decode_rle(data, &mut source[..arm * arm]) {
                    warn!("bad rle tile, skipped");
                    continue;
                }
            }
            KIND_PAL2 | KIND_PAL4 => {
                let bits = if tiles[head + 2] == KIND_PAL2 { 2 } else { 4 };
                if !decode_palette(data, &mut source[..arm * arm], bits) {
                    warn!("bad palette tile ({} bytes), skipped", data.len());
                    continue;
                }
            }
            KIND_RAW if data.len() == arm * arm * 2 => {
                for (dst, src) in source[..arm * arm].iter_mut().zip(data.as_chunks::<2>().0) {
                    *dst = u16::from_le_bytes([src[0], src[1]]);
                }
            }
            other => {
                warn!("bad tile encoding {} ({} bytes)", other, data.len());
                continue;
            }
        }

        // 每个源像素铺成 scale×scale，拼出一整块 16×16
        if scale == 1 {
            pixels.copy_from_slice(&source[..TILE * TILE]);
        } else {
            for y in 0..arm {
                for x in 0..arm {
                    let value = source[y * arm + x];
                    for dy in 0..scale {
                        let at = (y * scale + dy) * TILE + x * scale;
                        pixels[at..at + scale].fill(value);
                    }
                }
            }
        }

        let col = tiles[head] as u16;
        let row = tiles[head + 1] as u16;
        let x0 = col * TILE as u16;
        let y0 = row * TILE as u16;
        let started = Instant::now();
        display
            .draw_raw_slice(x0, y0, x0 + TILE as u16 - 1, y0 + TILE as u16 - 1, &pixels)
            .expect("tile draw failed");
        draw_us += started.elapsed().as_micros() as u64;
        hash = hash.wrapping_add(tile_hash(&tiles[head..body + len]));
        drawn += 1;
        // 每块都让出 CPU：UART 的 128 字节 FIFO 在 2 Mbaud 下只有 640 µs 余量，
        // 画一块的时长已经超过它，攒几块再让就会 Rx(FifoOverflowed) 丢整条消息
        yield_now().await;
    }
    (drawn, hash, draw_us)
}
