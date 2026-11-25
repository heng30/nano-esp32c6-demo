//! 上位机 ↔ 板子的命令协议，与固件 `ili9341-lcd-secondary-screen/src/link.rs` 一一对应。
//!
//! ```text
//! C6 D5 | cmd(1) | len(2, LE) | payload(len) | sum8(1)
//! ```
//!
//! 数据命令 `0x01` 传图像（载荷带帧号，板子靠它丢过期数据），控制命令 `0x02` 查询、
//! `0x03` 清屏、`0x04` 重置板子内部状态；板子只回 `0x81`：
//! `status + fps + busy_ms + frame + tiles + hash`。上位机目前只用 status/fps/busy_ms，
//! 后五个字节是板子本帧的进度与哈希，保留给诊断（`SCREEN_DEBUG=1` 可看原始应答）。

use std::{
    io::{Read, Write},
    time::Duration,
};

use serialport::SerialPort;

use crate::frame::{TILE, Tile};

/// 一条消息最多几块 tile，必须与固件一致（上位机按帧自适应，最大这个数）。
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

/// 控制命令：上位机确认收到几何信息。
const CMD_GEO_ACK: u8 = 0x06;
/// 控制命令：改写板子的采样倍数（载荷 1 字节）。
const CMD_SCALE: u8 = 0x05;

/// 板子的控制应答。
const RSP_REPORT: u8 = 0x81;
/// 板子上报的几何信息。
const RSP_GEOMETRY: u8 = 0x85;
/// 几何信息载荷长度：宽(2) + 高(2) + 倍数(1)。
const GEOMETRY_LEN: usize = 5;
/// 应答状态：处理完。
const ST_ACK: u8 = 0;
/// 应答状态：对查询的回应。
const ST_PONG: u8 = 2;

/// 批标志：本帧最后一批。
const LAST_BATCH: u8 = 0x01;

/// 串口超时：只作用于读与写。写满 1 秒说明板子彻底堵死（正常背压几十毫秒内就化开）；
/// 读不到 PONG 1 秒就当链路断了。
const TIMEOUT: Duration = Duration::from_secs(1);
/// 应答载荷长度（`status + fps + busy + frame + tiles + hash`），必须与固件一致。
const REPLY_MAX: usize = 12;
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

/// 板子的应答：状态 + 板子自测的帧率与忙碌毫秒；后面五个字节（帧号/块数/哈希）
/// 在连续推流下永远测不到终值，不解析，`SCREEN_DEBUG=1` 可看原始字节。
#[derive(Clone, Copy)]
pub struct Report {
    /// 应答状态。
    pub status: u8,
    /// 板子实测的帧率，0 表示还没测到（安静期刚过）。
    pub fps: u8,
    /// 本帧从收到第一块到画完最后一块花掉的毫秒数，0 表示还没整帧。
    pub busy_ms: u16,
}

impl Report {
    /// 板子已处理完这条命令。
    pub fn ok(&self) -> bool {
        let status = self.status & 0x7F;
        status == ST_ACK || status == ST_PONG
    }
}

/// 收发失败分两类：超时/坏应答是链路拥挤或瞬时干扰（降级即可），
/// 串口本体错误（写失败、非超时读错）是口已死。
pub enum LinkError {
    Timeout(String),
    Fatal(String),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Timeout(message) | LinkError::Fatal(message) => f.write_str(message),
        }
    }
}

/// 板子上报的几何信息。
#[derive(Clone, Copy)]
pub struct Geometry {
    /// 面板宽度（像素）。
    pub width: u16,
    /// 面板高度（像素）。
    pub height: u16,
    /// 采样倍数：抓屏取 `1/scale`，板子把每个像素铺成 `scale × scale`。
    pub scale: u8,
}

/// 串口连接与复用中的打包缓冲。
pub struct Link {
    port: Box<dyn SerialPort>,
    msg: Vec<u8>,
    /// 板子最近一次上报的几何信息（未确认前会持续上报）。
    geometry: Option<Geometry>,
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
            geometry: None,
        })
    }

    /// 数据命令：只管发，不等应答——板子对数据不回 ACK，丢了就丢了。
    ///
    /// `frame` 是帧号（板子靠它丢过期数据），`last` 标记本帧最后一块；
    /// `batch` / `total` 板子不解析，仅供诊断。写失败（含写超时：半截消息已把流
    /// 搞乱）一律按口已死处理。
    pub fn send_tiles(
        &mut self,
        frame: u16,
        batch: u8,
        total: u8,
        tiles: &[Tile],
        last: bool,
    ) -> Result<usize, LinkError> {
        self.msg.clear();
        self.msg.extend_from_slice(&SYNC);
        self.msg.push(CMD_TILES);
        // 长度字段先占位，tile 编码完再回填
        self.msg.extend_from_slice(&[0, 0]);
        self.msg.extend_from_slice(&frame.to_le_bytes());
        self.msg.push(if last { LAST_BATCH } else { 0 });
        self.msg.push(tiles.len() as u8);
        self.msg.push(batch);
        self.msg.push(total);
        for tile in tiles {
            encode_tile(tile, &mut self.msg);
        }
        let encoded = self.msg.len() - 2 - 3 - BATCH_HEAD;
        let len = (self.msg.len() - 5) as u16;
        self.msg[3..5].copy_from_slice(&len.to_le_bytes());
        let sum = self.msg[2..]
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        self.msg.push(sum);

        self.port
            .write_all(&self.msg)
            .map_err(|error| LinkError::Fatal(format!("serial write failed: {error}")))?;
        // 数据不 flush：tcdrain 要等内核缓冲完全排空，每条消息白付一次同步回环；
        // 内核缓冲满了 write_all 自己阻塞，就是背压
        Ok(encoded)
    }

    /// 控制命令：查询板子当前实测帧率。
    pub fn ping(&mut self) -> Result<Report, LinkError> {
        self.transact(CMD_PING, &[])
    }

    /// 控制命令：重置板子内部状态（帧号、进度、缓存、帧率采样），不动屏幕。
    pub fn reset(&mut self) -> Result<Report, LinkError> {
        self.transact(CMD_RESET, &[])
    }

    /// 控制命令：确认收到几何信息（不确认板子会一直重报）。
    pub fn ack_geometry(&mut self) -> Result<Report, LinkError> {
        self.transact(CMD_GEO_ACK, &[])
    }

    /// 板子最近上报的几何信息；还没收到过就是 `None`。
    pub fn geometry(&self) -> Option<Geometry> {
        self.geometry
    }

    /// 控制命令：改写板子的采样倍数，覆盖它自报的默认值。
    pub fn set_scale(&mut self, scale: u8) -> Result<Report, LinkError> {
        self.transact(CMD_SCALE, &[scale])
    }

    /// 控制命令：清屏。
    pub fn clear(&mut self) -> Result<Report, LinkError> {
        self.transact(CMD_CLEAR, &[])
    }

    /// 发一条命令并等一次应答：等不到就当丢了（流式数据缺块由后续帧补）。
    /// 不 flush：tcdrain 会等内核缓冲完全排空，每条消息白付一次同步回环。
    fn transact(&mut self, cmd: u8, payload: &[u8]) -> Result<Report, LinkError> {
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

        self.port
            .write_all(&self.msg)
            .map_err(|error| LinkError::Fatal(format!("serial write failed: {error}")))?;
        match self.read_report() {
            Ok(report) if report.ok() => Ok(report),
            // 迟到的上一条应答会被这次读到，只影响帧率数值，不影响图像
            Ok(report) => Err(LinkError::Timeout(format!(
                "board rejected: status {}",
                report.status
            ))),
            Err(error) => Err(error),
        }
    }

    /// 读固定字节数；超时是链路拥挤，其余是口已死。
    fn read_exact(&mut self, dst: &mut [u8]) -> Result<(), LinkError> {
        self.port.read_exact(dst).map_err(|error| {
            if error.kind() == std::io::ErrorKind::TimedOut {
                LinkError::Timeout(format!("read failed: {error}"))
            } else {
                LinkError::Fatal(format!("read failed: {error}"))
            }
        })
    }

    /// 扫同步字读一帧应答。
    fn read_report(&mut self) -> Result<Report, LinkError> {
        let mut byte = [0u8; 1];
        let mut prev = 0u8;
        loop {
            // 扫同步字
            loop {
                self.read_exact(&mut byte)?;
                if prev == SYNC[0] && byte[0] == SYNC[1] {
                    break;
                }
                prev = byte[0];
            }

            let mut body = [0u8; 3 + REPLY_MAX + 1];
            self.read_exact(&mut body[..3])?;
            let len = usize::from(u16::from_le_bytes([body[1], body[2]]));
            if body[0] == RSP_GEOMETRY && len == GEOMETRY_LEN {
                // 未确认前板子会一直重报，收到就记下，不影响本端等的是哪条应答
                self.read_exact(&mut body[3..3 + len + 1])?;
                let sum = body[..3 + len]
                    .iter()
                    .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
                if sum == body[3 + len] {
                    self.geometry = Some(Geometry {
                        width: u16::from_le_bytes([body[3], body[4]]),
                        height: u16::from_le_bytes([body[5], body[6]]),
                        scale: body[7],
                    });
                }
                prev = SYNC[1];
                continue;
            }
            if body[0] != RSP_REPORT {
                // 坏应答多半是字节流错位/损坏，重发后重新扫同步字即可
                return Err(LinkError::Timeout(format!(
                    "unexpected reply 0x{:02x}",
                    body[0]
                )));
            }
            if len != REPLY_MAX {
                return Err(LinkError::Timeout(format!("bad reply length {len}")));
            }
            self.read_exact(&mut body[3..3 + len + 1])?;

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
                return Err(LinkError::Timeout("bad reply checksum".to_string()));
            }

            return Ok(Report {
                status: body[3] & 0x7F,
                fps: body[4],
                busy_ms: u16::from_le_bytes([body[5], body[6]]),
            });
        }
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

    // 放大时一块里只有 (16/scale)² 个源像素，板子按 scale 铺回 16×16
    let arm = tile.arm as usize;
    let pixels = &tile.pixels[..arm * arm];
    let raw_len = arm * arm * 2;
    encode_rle(pixels, out);
    let len = out.len() - head - 3;
    if len >= raw_len {
        out.truncate(head);
        out.push(KIND_RAW);
        out.extend_from_slice(&(raw_len as u16).to_le_bytes());
        for pixel in pixels {
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
            arm: TILE as u8,
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
            arm: TILE as u8,
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
                busy_ms: 0
            }
            .ok()
        );
        assert!(
            Report {
                status: ST_PONG,
                fps: 7,
                busy_ms: 12
            }
            .ok()
        );
        assert!(
            !Report {
                status: 1,
                fps: 0,
                busy_ms: 0
            }
            .ok()
        );
    }
}
