//! 上位机 ↔ 板子的命令协议，与固件 `ili9341-lcd-secondary-screen/src/link.rs` 一一对应。
//!
//! ```text
//! C6 D5 | cmd(1) | len(2, LE) | payload(len) | sum8(1)
//! ```
//!
//! 数据命令 `0x01` 传图像（载荷带帧号，板子靠它丢过期数据），控制命令 `0x02` 查询、
//! `0x03` 清屏、`0x04` 重置板子内部状态；板子只回 `0x81`：
//! `status + fps + busy_ms + frame + tiles + hash`，最后两个是板子本帧的进度与数据哈希，
//! 上位机按帧核对，不需要看屏幕就知道板子收全了没有。

use std::{
    io::{Read, Write},
    time::Duration,
};

use serialport::SerialPort;

use crate::frame::{TILE, Tile};

/// 一批最多几个 tile，必须与固件一致。
const MAX_TILES: usize = 32;
/// 数据载荷头：帧号、标志、本批块数、批序号、批总数。
const BATCH_HEAD: usize = 6;
/// 消息头的同步字。
const SYNC: [u8; 2] = [0xC6, 0xD5];

/// 数据命令：一批 tile。
const CMD_TILES: u8 = 0x01;
/// 控制命令：查询板子实测帧率。
const CMD_PING: u8 = 0x02;
/// 控制命令：清屏。
const CMD_CLEAR: u8 = 0x03;
/// 控制命令：重置板子内部状态（帧号、进度、缓存、帧率采样）。
const CMD_RESET: u8 = 0x04;

/// 板子的控制应答。
const RSP_REPORT: u8 = 0x81;
/// 应答状态：处理完。
const ST_ACK: u8 = 0;
/// 应答状态：对查询的回应。
const ST_PONG: u8 = 2;

/// 批标志：本帧最后一批。
const LAST_BATCH: u8 = 0x01;

/// 等应答的超时。板子画 8 块约 3 ms，1 秒只是给串口排队留的余量。
const TIMEOUT: Duration = Duration::from_millis(400);
/// 一条消息最多重发几次。
const RETRY: usize = 2;
/// 应答载荷长度（`status + fps + busy + frame + tiles + hash`），必须与固件一致。
const REPLY_MAX: usize = 12;
/// 一块数据的哈希（FNV-1a），与固件同一算法，用来核对板子收到的字节。
fn tile_hash(bytes: &[u8]) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// 一块 tile 的像素字节数。
const RAW_LEN: usize = TILE * TILE * 2;
/// 一块 tile 的编码：原样。
const KIND_RAW: u8 = 0;
/// 一块 tile 的编码：游程。
const KIND_RLE: u8 = 1;
/// 一块 tile 的头：列、行、编码、载荷长度（u16）。
const TILE_HEAD: usize = 5;
/// 一条消息的最大字节数（不含同步字）。
const MSG_MAX: usize = 3 + BATCH_HEAD + MAX_TILES * (TILE_HEAD + RAW_LEN) + 1;

/// 板子的应答。
#[derive(Clone, Copy)]
pub struct Report {
    /// 应答状态。
    pub status: u8,
    /// 板子实测的帧率，0 表示还没测到（安静期刚过）。
    pub fps: u8,
    /// 本帧从收到第一批到画完最后一批花掉的毫秒数，0 表示还没整帧。
    pub busy_ms: u16,
    /// 板子当前的帧号。
    pub frame: u16,
    /// 板子本帧已应用的块数。
    pub tiles: u16,
    /// 板子本帧已应用数据的逐块哈希之和。
    pub hash: u32,
    /// 板子本帧已经画完（计数是最终值）。ACK 是入队即回，没画完时校验没意义。
    pub complete: bool,
}

impl Report {
    /// 板子已处理完这条命令。
    pub fn ok(&self) -> bool {
        let status = self.status & 0x7F;
        status == ST_ACK || status == ST_PONG
    }
}

/// 串口连接与复用中的打包缓冲。
pub struct Link {
    port: Box<dyn SerialPort>,
    msg: Vec<u8>,
}

impl Link {
    pub fn open(path: &str, baud: u32) -> Result<Self, String> {
        let mut port = serialport::new(path, baud)
            .timeout(TIMEOUT)
            .open()
            .map_err(|error| format!("open {path} failed: {error}"))?;
        // 拉低 DTR/RTS：ESP 板子的自动复位电路把这两根线接到 EN/IO0，
        // 打开串口时把它们拉高会把芯片按在复位里，UART0 链路就永远没有应答
        let _ = port.write_data_terminal_ready(false);
        let _ = port.write_request_to_send(false);
        Ok(Self {
            port,
            msg: Vec::with_capacity(MSG_MAX),
        })
    }

    /// 数据命令：发一批 tile。返回应答、这批载荷的字节数、这批数据的帧哈希。
    ///
    /// `frame` 是帧号（板子用它丢过期数据），`batch` / `total` 是本批在本帧里的序号与总批数，
    /// `last` 标记本帧最后一批。上位机把所有批的哈希加起来，和板子回的 `hash` 核对。
    pub fn send_tiles(
        &mut self,
        frame: u16,
        batch: u8,
        total: u8,
        tiles: &[Tile],
        last: bool,
    ) -> Result<(Report, usize, u32), String> {
        self.msg.clear();
        self.msg.extend_from_slice(&frame.to_le_bytes());
        self.msg.push(if last { LAST_BATCH } else { 0 });
        self.msg.push(tiles.len() as u8);
        self.msg.push(batch);
        self.msg.push(total);

        let mut hash = 0u32;
        for tile in tiles {
            let start = self.msg.len();
            encode_tile(tile, &mut self.msg);
            hash = hash.wrapping_add(tile_hash(&self.msg[start..]));
        }
        let encoded = self.msg.len() - BATCH_HEAD;
        let payload = std::mem::take(&mut self.msg);
        let report = self.transact(CMD_TILES, &payload);
        self.msg = payload;
        report.map(|report| (report, encoded, hash))
    }

    /// 控制命令：查询板子当前实测帧率。
    pub fn ping(&mut self) -> Result<Report, String> {
        self.transact(CMD_PING, &[])
    }

    /// 控制命令：重置板子内部状态（帧号、进度、缓存、帧率采样），不动屏幕。
    pub fn reset(&mut self) -> Result<Report, String> {
        self.transact(CMD_RESET, &[])
    }

    /// 控制命令：清屏。
    pub fn clear(&mut self) -> Result<Report, String> {
        self.transact(CMD_CLEAR, &[])
    }

    /// 发一条命令并等应答，重试 `RETRY` 次仍失败就返回最后一次的错误。
    fn transact(&mut self, cmd: u8, payload: &[u8]) -> Result<Report, String> {
        self.msg.clear();
        self.msg.extend_from_slice(&SYNC);
        self.msg.push(cmd);
        self.msg
            .extend_from_slice(&(payload.len() as u16).to_le_bytes());
        self.msg.extend_from_slice(payload);
        let sum = self.msg[2..]
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        self.msg.push(sum);

        if std::env::var_os("SCREEN_DEBUG").is_some() {
            eprintln!(
                "tx cmd 0x{:02x} len {} sum 0x{:02x}",
                self.msg[2],
                payload.len(),
                self.msg[self.msg.len() - 1]
            );
        }

        let mut last = String::new();
        for _ in 0..RETRY {
            self.port
                .write_all(&self.msg)
                .map_err(|error| format!("serial write failed: {error}"))?;
            self.port
                .flush()
                .map_err(|error| format!("serial flush failed: {error}"))?;
            match self.read_report() {
                Ok(report) if report.ok() => return Ok(report),
                // 重发后可能读到上一条命令的重复应答，只影响帧率数值，不影响图像
                Ok(report) => last = format!("board rejected: status {}", report.status),
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    /// 扫同步字读一帧应答。
    fn read_report(&mut self) -> Result<Report, String> {
        let mut byte = [0u8; 1];
        let mut prev = 0u8;
        loop {
            self.port
                .read_exact(&mut byte)
                .map_err(|error| format!("no reply: {error}"))?;
            if prev == SYNC[0] && byte[0] == SYNC[1] {
                break;
            }
            prev = byte[0];
        }

        let mut body = [0u8; 3 + REPLY_MAX + 1];
        self.port
            .read_exact(&mut body[..3])
            .map_err(|error| format!("no reply header: {error}"))?;
        if body[0] != RSP_REPORT {
            return Err(format!("unexpected reply 0x{:02x}", body[0]));
        }
        let len = usize::from(u16::from_le_bytes([body[1], body[2]]));
        if len != REPLY_MAX {
            return Err(format!("bad reply length {len}"));
        }
        self.port
            .read_exact(&mut body[3..3 + len + 1])
            .map_err(|error| format!("short reply: {error}"))?;

        if std::env::var_os("SCREEN_DEBUG").is_some() {
            eprintln!(
                "rx cmd 0x{:02x} len {len} body {:02x?} sum 0x{:02x}",
                body[0],
                &body[3..3 + len],
                body[3 + len]
            );
        }

        let sum = body[..3 + len]
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        if sum != body[3 + len] {
            return Err("bad reply checksum".to_string());
        }

        Ok(Report {
            status: body[3] & 0x7F,
            fps: body[4],
            busy_ms: u16::from_le_bytes([body[5], body[6]]),
            frame: u16::from_le_bytes([body[7], body[8]]),
            tiles: u16::from_le_bytes([body[9], body[10]]),
            hash: u32::from_le_bytes([body[11], body[12], body[13], body[14]]),
            complete: body[3] & 0x80 != 0,
        })
    }
}

/// 一块 tile 的载荷：`col, row, kind, len(u16 LE), data`。
///
/// 游程编码更短就用游程，否则原样发——保证任何内容都不会比原来更大。
fn encode_tile(tile: &Tile, out: &mut Vec<u8>) {
    out.push(tile.col);
    out.push(tile.row);
    let head = out.len();
    out.extend_from_slice(&[KIND_RLE, 0, 0]);

    encode_rle(&tile.pixels, out);
    let len = out.len() - head - 3;
    if len >= RAW_LEN {
        out.truncate(head);
        out.push(KIND_RAW);
        out.extend_from_slice(&(RAW_LEN as u16).to_le_bytes());
        for pixel in &tile.pixels {
            out.extend_from_slice(&pixel.to_le_bytes());
        }
    } else {
        out[head + 1..head + 3].copy_from_slice(&(len as u16).to_le_bytes());
    }
}

/// 游程编码：最高位 1 = 一个颜色重复 n 次（1 个 u16），否则 n 个像素原样（n ≤ 128）。
fn encode_rle(pixels: &[u16], out: &mut Vec<u8>) {
    let mut index = 0;
    while index < pixels.len() {
        let run = same_run(pixels, index);
        if run >= 3 {
            out.push(0x80 | (run - 1) as u8);
            out.extend_from_slice(&pixels[index].to_le_bytes());
            index += run;
            continue;
        }

        // 字面量：攒到下一个够长的重复段或攒满 128 个
        let begin = index;
        index += run;
        while index < pixels.len() && index - begin < 128 && same_run(pixels, index) < 3 {
            index += same_run(pixels, index);
        }
        let count = (index - begin).min(128);
        out.push((count - 1) as u8);
        for pixel in &pixels[begin..begin + count] {
            out.extend_from_slice(&pixel.to_le_bytes());
        }
        index = begin + count;
    }
}

/// 从 `index` 起有多少个相同的像素（最多 128 个）。
fn same_run(pixels: &[u16], index: usize) -> usize {
    let mut run = 1;
    while index + run < pixels.len() && run < 128 && pixels[index + run] == pixels[index] {
        run += 1;
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 组一帧数据命令，不用真串口。
    fn encode(cmd: u8, payload: &[u8]) -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend_from_slice(&SYNC);
        msg.push(cmd);
        msg.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        msg.extend_from_slice(payload);
        let sum = msg[2..]
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        msg.push(sum);
        msg
    }

    fn tile(col: u8, row: u8, value: u16) -> Tile {
        Tile {
            col,
            row,
            pixels: [value; TILE * TILE],
        }
    }

    #[test]
    fn tile_payload_starts_with_flags_and_checksums() {
        let tiles = [tile(3, 1, 0x1234)];

        // 与 `Link::send_tiles` 一致的载荷布局
        let mut payload = vec![1 | LAST_BATCH];
        for t in &tiles {
            payload.push(t.col);
            payload.push(t.row);
            for pixel in &t.pixels {
                payload.extend_from_slice(&pixel.to_le_bytes());
            }
        }

        let msg = encode(CMD_TILES, &payload);
        assert_eq!(msg.len(), 2 + 3 + payload.len() + 1);
        assert_eq!(&msg[..2], &SYNC);
        assert_eq!(msg[2], CMD_TILES);
        assert_eq!(u16::from_le_bytes([msg[3], msg[4]]), payload.len() as u16);
        // 载荷首字节同时带 tile 数与「最后一批」标志
        assert_eq!(msg[5], 1 | LAST_BATCH);
        assert_eq!(&msg[6..8], &[3, 1]);
        assert_eq!(&msg[8..10], &0x1234u16.to_le_bytes());

        let sum = msg[2..msg.len() - 1]
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        assert_eq!(msg[msg.len() - 1], sum);
    }

    /// 按固件 `draw_tiles` 的解析方式还原一块 tile 的像素。
    fn decode(payload: &[u8]) -> [u16; TILE * TILE] {
        let len = usize::from(u16::from_le_bytes([payload[3], payload[4]]));
        let body = &payload[5..5 + len];
        let mut pixels = [0u16; TILE * TILE];

        if payload[2] == KIND_RAW {
            assert_eq!(len, RAW_LEN);
            for (dst, src) in pixels.iter_mut().zip(body.as_chunks::<2>().0) {
                *dst = u16::from_le_bytes(*src);
            }
            return pixels;
        }

        let mut at = 0;
        let mut index = 0;
        while at < body.len() {
            let tag = body[at];
            at += 1;
            let count = usize::from(tag & 0x7F) + 1;
            if tag & 0x80 != 0 {
                let pixel = u16::from_le_bytes([body[at], body[at + 1]]);
                at += 2;
                pixels[index..index + count].fill(pixel);
            } else {
                for slot in &mut pixels[index..index + count] {
                    *slot = u16::from_le_bytes([body[at], body[at + 1]]);
                    at += 2;
                }
            }
            index += count;
        }
        assert_eq!(index, TILE * TILE);
        pixels
    }

    /// 平铺同一颜色的 tile 用游程。
    fn flat_tile(col: u8, row: u8, value: u16) -> Tile {
        Tile {
            col,
            row,
            pixels: [value; TILE * TILE],
        }
    }

    #[test]
    fn flat_tile_compresses_hard() {
        let mut msg = Vec::new();
        encode_tile(&flat_tile(3, 1, 0x1234), &mut msg);

        assert_eq!(msg[2], KIND_RLE);
        assert!(msg.len() < 16, "flat tile took {} bytes", msg.len());
        assert!(decode(&msg).iter().all(|pixel| *pixel == 0x1234));
    }

    #[test]
    fn noisy_tile_falls_back_to_raw() {
        let mut tile = flat_tile(0, 0, 0);
        for (index, pixel) in tile.pixels.iter_mut().enumerate() {
            *pixel = (index as u16).wrapping_mul(7919);
        }

        let mut msg = Vec::new();
        encode_tile(&tile, &mut msg);

        assert_eq!(msg[2], KIND_RAW);
        assert_eq!(msg.len(), TILE_HEAD + RAW_LEN);
        assert_eq!(decode(&msg), tile.pixels);
    }

    #[test]
    fn report_status_gate() {
        assert!(
            Report {
                status: ST_ACK,
                fps: 0,
                busy_ms: 0,
                frame: 0,
                tiles: 0,
                hash: 0,
            }
            .ok()
        );
        assert!(
            Report {
                status: ST_PONG,
                fps: 7,
                busy_ms: 12,
                frame: 3,
                tiles: 5,
                hash: 0
            }
            .ok()
        );
        assert!(
            !Report {
                status: 1,
                fps: 0,
                busy_ms: 0,
                frame: 0,
                tiles: 0,
                hash: 0
            }
            .ok()
        );
    }
}
