//! 上位机 ↔ 板子的命令协议。
//!
//! 统一信封（多字节整数一律小端）：
//!
//! ```text
//! C6 D5 | cmd(1) | len(2, LE) | payload(len) | sum8(1)
//! ```
//!
//! `sum8` 是 `cmd` 到载荷最后一字节的逐字节和（u8 回绕）。命令分两类：
//!
//! - **数据命令** `0x0X`（上位机 → 板子）：`CMD_TILES` 传一块或一批 16×16 的 RGB565 图像
//! - **控制命令** `0x0X`（上位机 → 板子）：`CMD_PING` 查询、`CMD_CLEAR` 清屏
//! - **应答** `0x81`（板子 → 上位机）：`status + fps`，板子只发这一类控制帧
//!
//! 严格一问一答：板子回一帧后上位机才发下一条，所以接收缓冲只需装得下一条消息，
//! 画屏期间也不会有数据继续涌来。上位机的实现见 `desktop/src/link.rs`，两边常量必须一起改。

use defmt::{Format, info, warn};
use embedded_graphics::{pixelcolor::RgbColor, prelude::*};
use embedded_io::{Read, Write};
use esp_hal::time::Instant;
use static_cell::StaticCell;

use crate::display::Display;

/// 一个 tile 的边长（像素）。
const TILE: usize = 16;
/// 一批最多几个 tile。越大，每条消息的固定开销（同步、校验、等应答）摊得越薄，
/// 代价是静态接收缓冲 33 KB。
const MAX_TILES: usize = 64;
/// 一块 tile 的像素字节数。
const RAW_LEN: usize = TILE * TILE * 2;
/// 一块 tile 的编码：原样。
const KIND_RAW: u8 = 0;
/// 一块 tile 的编码：游程（最高位 1 = 重复 n 次同色，否则 n 个像素原样，n ≤ 128）。
const KIND_RLE: u8 = 1;
/// 一块 tile 的头：列、行、编码、载荷长度（u16）。
const TILE_HEAD: usize = 5;
/// `CMD_TILES` 载荷的最大长度：标志位 + 一批 tile。
const PAYLOAD_MAX: usize = 1 + MAX_TILES * (TILE_HEAD + RAW_LEN);
/// 一条消息的最大字节数（不含同步字）：`cmd + len + payload + sum`。
const MSG_MAX: usize = 3 + PAYLOAD_MAX + 1;
/// 消息头的同步字。
const SYNC: [u8; 2] = [0xC6, 0xD5];

/// 数据命令：一批 tile。
const CMD_TILES: u8 = 0x01;
/// 控制命令：查询，板子回 `ST_PONG`。
const CMD_PING: u8 = 0x02;
/// 控制命令：清屏。
const CMD_CLEAR: u8 = 0x03;

/// 板子的控制应答。
const RSP_REPORT: u8 = 0x81;
/// 应答状态：收到并处理完。
const ST_ACK: u8 = 0;
/// 应答状态：校验或命令错，上位机应重发。
const ST_NAK: u8 = 1;
/// 应答状态：对 `CMD_PING` 的回应。
const ST_PONG: u8 = 2;
/// 应答载荷长度：状态 + 帧率 + 本帧忙碌毫秒数。
const REPORT_LEN: u16 = 4;

/// `CMD_TILES` 载荷首字节的最高位：本帧最后一批，用来统计实际帧率。
const LAST_BATCH: u8 = 0x80;
/// 上报帧率的上限，与上位机一致。
const MAX_FPS: u8 = 30;
/// 超过这么久没收到整帧就丢掉帧率样本，免得安静期把间隔算进下一帧。
const IDLE_MS: u64 = 2000;
/// 每收多少 tile 打一条日志（全屏 300 块，约等于两帧一次）。
const LOG_EVERY: u32 = 600;

/// 接收缓冲，4 KB 放静态区，别压在任务栈上。
static MSG: StaticCell<[u8; MSG_MAX]> = StaticCell::new();

/// 从链路搬字节的小缓冲：SYNC 扫描逐字节，载荷整块拷。
struct Reader {
    buf: [u8; 128],
    pos: usize,
    len: usize,
}

impl Reader {
    const fn new() -> Self {
        Self {
            buf: [0; 128],
            pos: 0,
            len: 0,
        }
    }

    fn byte<T: Read>(&mut self, link: &mut T) -> Result<u8, T::Error> {
        if self.pos == self.len {
            self.len = link.read(&mut self.buf)?;
            self.pos = 0;
        }
        let byte = self.buf[self.pos];
        self.pos += 1;
        Ok(byte)
    }

    /// 拷满 `dst`：先吃缓冲里剩下的，不够再直接从链路读到目标缓冲。
    fn read_into<T: Read>(&mut self, link: &mut T, dst: &mut [u8]) -> Result<(), T::Error> {
        let rest = self.len - self.pos;
        let n = rest.min(dst.len());
        dst[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;

        let mut filled = n;
        while filled < dst.len() {
            filled += link.read(&mut dst[filled..])?;
        }
        Ok(())
    }
}

/// 扫到同步字后读一条完整消息，返回消息长度（`cmd` 到 `sum8`）；`0` 表示长度非法、已丢弃。
fn read_message<T: Read>(
    reader: &mut Reader,
    link: &mut T,
    msg: &mut [u8],
) -> Result<usize, T::Error> {
    let mut prev = 0u8;
    loop {
        let byte = reader.byte(link)?;
        if prev == SYNC[0] && byte == SYNC[1] {
            break;
        }
        prev = byte;
    }

    reader.read_into(link, &mut msg[..3])?;
    let len = usize::from(u16::from_le_bytes([msg[1], msg[2]]));
    if 3 + len + 1 > msg.len() {
        return Ok(0);
    }

    reader.read_into(link, &mut msg[3..3 + len + 1])?;
    Ok(3 + len + 1)
}

/// 校验整条消息的字节和。
fn checksum_ok(msg: &[u8]) -> bool {
    let sum = msg[..msg.len() - 1]
        .iter()
        .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
    sum == msg[msg.len() - 1]
}

/// 按顺序走一遍 tile 头，校验偏移与长度都落在载荷内；返回每块的数据区间。
fn tile_spans(
    payload: &[u8],
    count: usize,
) -> Option<impl Iterator<Item = (usize, usize, usize)> + '_> {
    let mut spans = [0usize; MAX_TILES * 3];
    let mut at = 1;
    for index in 0..count {
        let head = at;
        if head + TILE_HEAD > payload.len() {
            return None;
        }
        let len = usize::from(u16::from_le_bytes([payload[head + 3], payload[head + 4]]));
        let body = head + TILE_HEAD;
        if body + len > payload.len() {
            return None;
        }
        spans[index * 3] = head;
        spans[index * 3 + 1] = body;
        spans[index * 3 + 2] = len;
        at = body + len;
    }
    (at == payload.len()).then(|| {
        (0..count).map(move |index| {
            let head = spans[index * 3];
            (head, spans[index * 3 + 1], spans[index * 3 + 2])
        })
    })
}

/// 把一条数据命令里的 tile 逐块写到屏上。
fn draw_tiles(display: &mut Display, payload: &[u8], count: usize) {
    let Some(spans) = tile_spans(payload, count) else {
        warn!("bad tile layout in batch of {}", count);
        return;
    };

    let mut pixels = [0u16; TILE * TILE];
    for (head, body, len) in spans {
        let data = &payload[body..body + len];
        match payload[head + 2] {
            KIND_RLE => {
                if !decode_rle(data, &mut pixels) {
                    warn!("bad rle tile, skipped");
                    continue;
                }
            }
            KIND_RAW if data.len() == RAW_LEN => {
                for (dst, src) in pixels.iter_mut().zip(data.as_chunks::<2>().0) {
                    *dst = u16::from_le_bytes([src[0], src[1]]);
                }
            }
            other => {
                warn!("bad tile encoding {} ({} bytes)", other, data.len());
                continue;
            }
        }

        let col = payload[head] as u16;
        let row = payload[head + 1] as u16;
        let x0 = col * TILE as u16;
        let y0 = row * TILE as u16;
        display
            .draw_raw_slice(x0, y0, x0 + TILE as u16 - 1, y0 + TILE as u16 - 1, &pixels)
            .expect("tile draw failed");
    }
}

/// 解游程编码到 256 个像素；长度对不上返回 `false`。
fn decode_rle(data: &[u8], pixels: &mut [u16; TILE * TILE]) -> bool {
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
            pixels[index..index + count].fill(pixel);
        } else {
            if at + count * 2 > data.len() {
                return false;
            }
            for slot in &mut pixels[index..index + count] {
                *slot = u16::from_le_bytes([data[at], data[at + 1]]);
                at += 2;
            }
        }
        index += count;
    }
    index == pixels.len()
}

/// 回一帧应答：`status + fps`。
fn reply<T: Write>(link: &mut T, status: u8, fps: u8, busy_ms: u16)
where
    T::Error: Format,
{
    let mut frame = [0u8; 10];
    frame[0..2].copy_from_slice(&SYNC);
    frame[2] = RSP_REPORT;
    frame[3..5].copy_from_slice(&REPORT_LEN.to_le_bytes());
    frame[5] = status;
    frame[6] = fps;
    frame[7..9].copy_from_slice(&busy_ms.to_le_bytes());
    frame[9] = frame[2..9]
        .iter()
        .fold(0u8, |acc, byte| acc.wrapping_add(*byte));

    if let Err(error) = link.write_all(&frame) {
        warn!("reply write failed: {:?}", error);
    }
}

/// 板子侧测到的帧率，用「每秒千分之一帧」的定点数平滑。
struct Rate {
    /// 上一帧的完成时刻。
    last: Option<Instant>,
    /// 平滑后的帧率 × 1000。
    value: u32,
}

impl Rate {
    const fn new() -> Self {
        Self {
            last: None,
            value: 0,
        }
    }

    /// 上报值：四舍五入到整数帧；0 表示还没测到，或上一次采样已经过期（安静期）。
    fn fps(&self) -> u8 {
        match self.last {
            Some(last) if last.elapsed().as_millis() <= IDLE_MS => {}
            _ => return 0,
        }
        ((self.value + 500) / 1000).min(u32::from(MAX_FPS)) as u8
    }

    /// 收完一整帧时调用；间隔太久（安静期）就丢掉旧样本重新开始。
    fn mark(&mut self) {
        let now = Instant::now();
        if let Some(previous) = self.last {
            let raw = previous.elapsed().as_millis();
            let ms = raw.max(1) as u32;
            self.value = if raw > IDLE_MS {
                0
            } else if self.value == 0 {
                1_000_000 / ms
            } else {
                (self.value * 3 + 1_000_000 / ms) / 4
            };
        }
        self.last = Some(now);
    }
}

/// 收帧主循环：永不返回，靠上位机停流。
///
/// 只接受一条链路（`main` 里给的 USB CDC），这里只要求实现阻塞的 `embedded_io::{Read, Write}`。
pub fn run<T>(link: &mut T, display: &mut Display) -> !
where
    T: Read + Write,
    T::Error: Format,
{
    let msg = MSG.init([0; MSG_MAX]);
    // 缓冲跨消息复用：一次读很可能越过本条消息的结尾，吞进来的下一条消息开头不能丢，
    // 否则整批 tile 被跳过 —— 屏上就是缺块、错块。
    let mut reader = Reader::new();
    let mut rate = Rate::new();
    let mut tiles = 0u32;
    let mut frame_start: Option<Instant> = None;
    let mut busy_ms: u16 = 0;

    loop {
        let total = match read_message(&mut reader, link, msg) {
            Ok(0) => {
                warn!("bad length, message dropped");
                continue;
            }
            Ok(total) => total,
            Err(error) => {
                warn!("read failed: {:?}", error);
                continue;
            }
        };

        if !checksum_ok(&msg[..total]) {
            warn!("bad checksum, message dropped");
            reply(link, ST_NAK, rate.fps(), busy_ms);
            continue;
        }

        let payload = &msg[3..total - 1];
        match msg[0] {
            CMD_TILES => {
                let count = usize::from(payload[0] & !LAST_BATCH);
                if !(1..=MAX_TILES).contains(&count) || tile_spans(payload, count).is_none() {
                    warn!("bad tile batch: count {}", count);
                    reply(link, ST_NAK, rate.fps(), busy_ms);
                    continue;
                }

                if frame_start.is_none() {
                    frame_start = Some(Instant::now());
                }

                draw_tiles(display, payload, count);
                tiles += count as u32;

                // 本帧从第一批到画完最后一批占了多少毫秒，上位机据此算板子的能力上限
                if payload[0] & LAST_BATCH != 0 {
                    if let Some(start) = frame_start.take() {
                        // 至少算 1 ms：单批 1 块的帧真实耗时不足 1 ms，报 0 会让上位机以为是「没数据」
                        busy_ms = start.elapsed().as_millis().clamp(1, u64::from(u16::MAX)) as u16;
                    }
                    rate.mark();
                }
                if tiles >= LOG_EVERY {
                    info!("tiles received: {}, fps {}", tiles, rate.fps());
                    tiles = 0;
                }

                reply(link, ST_ACK, rate.fps(), busy_ms);
            }
            CMD_PING => reply(link, ST_PONG, rate.fps(), busy_ms),
            CMD_CLEAR => {
                display
                    .clear(embedded_graphics::pixelcolor::Rgb565::BLACK)
                    .ok();
                reply(link, ST_ACK, rate.fps(), busy_ms);
            }
            other => {
                warn!("unknown command {}, dropped", other);
                reply(link, ST_NAK, rate.fps(), busy_ms);
            }
        }
    }
}
