//! 上位机 ↔ 板子的命令协议（v2）。
//!
//! 信封（多字节整数一律小端）：
//!
//! ```text
//! C6 D5 | cmd(1) | len(2, LE) | payload(len) | sum8(1)
//! ```
//!
//! 数据命令 `CMD_TILES` 的载荷：
//!
//! ```text
//! frame(2) | flags(1) | batch(1) | total(1) | tile*
//! ```
//!
//! - `frame`：帧号，板子靠它丢过期数据、认出「下一帧」
//! - `flags` 最低位 = 本帧最后一批
//! - `batch` / `total`：本批在本帧里的序号与总批数
//!
//! 应答 `RSP_REPORT` 的载荷：`status(1) | fps(1) | frame(2) | tiles(2) | hash(4)`。
//! `tiles` 是本帧已应用的块数，`hash` 是本帧已应用数据的逐块哈希之和——上位机拿它自校验，
//! 不看屏幕就知道板子有没有收全。上位机实现见 `desktop/src/link.rs`，两边常量必须一起改。

use defmt::{Format, info, warn};
use embassy_futures::yield_now;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, mutex::Mutex,
};
use embedded_graphics::{pixelcolor::RgbColor, prelude::*};
use embedded_io_async::{Read, Write};
use esp_hal::time::Instant;
use static_cell::StaticCell;

use crate::display::Display;

/// 一个 tile 的边长（像素）。
const TILE: usize = 16;
/// 一条消息最多几块 tile：上位机按帧自适应（小帧 1 块、大帧最多 32 块）。
const MAX_TILES: usize = 32;
/// 一块 tile 的像素字节数。
const RAW_LEN: usize = TILE * TILE * 2;
/// 一块 tile 的编码：原样。
const KIND_RAW: u8 = 0;
/// 一块 tile 的编码：游程（最高位 1 = 重复 n 次同色，否则 n 个像素原样，n ≤ 128）。
const KIND_RLE: u8 = 1;
/// 一块 tile 的头：列、行、编码、载荷长度（u16）。
const TILE_HEAD: usize = 5;
/// 数据载荷头：帧号、标志、本批块数、批序号、批总数。
const BATCH_HEAD: usize = 6;
/// `CMD_TILES` 载荷的最大长度：批头 + 一批 tile。
const PAYLOAD_MAX: usize = BATCH_HEAD + MAX_TILES * (TILE_HEAD + RAW_LEN);
/// 一条消息的最大字节数（不含同步字）：`cmd + len + payload + sum`。
const MSG_MAX: usize = 3 + PAYLOAD_MAX + 1;
/// 消息头的同步字。
const SYNC: [u8; 2] = [0xC6, 0xD5];

/// 数据命令：一批 tile。
const CMD_TILES: u8 = 0x01;
/// 控制命令：查询。
const CMD_PING: u8 = 0x02;
/// 控制命令：清屏。
const CMD_CLEAR: u8 = 0x03;
/// 控制命令：重置板子内部状态（帧号、进度、缓存、帧率采样），不动屏幕。
const CMD_RESET: u8 = 0x04;

/// 板子的控制应答。
const RSP_REPORT: u8 = 0x81;
/// 应答状态：收到并处理完。
const ST_ACK: u8 = 0;
/// 应答状态：校验或命令错，上位机应重发。
const ST_NAK: u8 = 1;
/// 应答状态：对 `CMD_PING` 的回应。
const ST_PONG: u8 = 2;
/// 应答载荷长度：状态 + 帧率 + 忙碌毫秒 + 帧号 + 本帧块数 + 本帧哈希。
const REPORT_LEN: u16 = 12;
/// 一条应答的总字节数：同步 + 命令 + 长度 + 载荷 + 校验。
const REPLY_MAX: usize = 2 + 3 + REPORT_LEN as usize + 1;

/// 批标志：本帧最后一批。
const LAST_BATCH: u8 = 0x01;
/// 应答状态的最高位：本帧已画完（计数是最终值）。
const ST_COMPLETE: u8 = 0x80;
/// 上报帧率的上限，与上位机一致。
const MAX_FPS: u8 = 30;
/// 超过这么久没收到整帧就丢掉帧率样本，免得安静期把间隔算进下一帧。
const IDLE_MS: u64 = 2000;
/// 每收多少 tile 打一条日志。
const LOG_EVERY: u32 = 2000;


/// 槽池：每条消息一块（最大约 4 KB）；13 块刚好装得下一整帧（300 块 / 32 块一条 = 10 条）
/// 加上两条链路各在手的一块与渲染在手的一块。
/// 收帧任务补不到块就不读下一条：链路背压全靠这一点（USB 有流控字节顶在缓冲里，
/// UART 没有会溢出丢掉，流式数据可接受）。
pub const POOL_BLOCKS: usize = 13;
static POOL: StaticCell<[u8; MSG_MAX * POOL_BLOCKS]> = StaticCell::new();
/// 空闲槽表。
static FREE: Channel<CriticalSectionRawMutex, &'static mut [u8], POOL_BLOCKS> = Channel::new();
/// 待渲染队列：收到块就投进来，ACK 等渲染腾出下一块槽再回。
static QUEUE: Channel<CriticalSectionRawMutex, Block, POOL_BLOCKS> = Channel::new();
/// 渲染进度快照，收发任务回 ACK 时读它。
static SNAP: Mutex<CriticalSectionRawMutex, Snap> = Mutex::new(Snap::new());

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

    async fn byte<T: Read>(&mut self, link: &mut T) -> Result<u8, T::Error> {
        if self.pos == self.len {
            self.len = link.read(&mut self.buf).await?;
            self.pos = 0;
        }
        let byte = self.buf[self.pos];
        self.pos += 1;
        Ok(byte)
    }

    /// 拷满 `dst`：先吃缓冲里剩下的，不够再直接从链路读到目标缓冲。
    async fn read_into<T: Read>(&mut self, link: &mut T, dst: &mut [u8]) -> Result<(), T::Error> {
        let rest = self.len - self.pos;
        let n = rest.min(dst.len());
        dst[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;

        let mut filled = n;
        while filled < dst.len() {
            filled += link.read(&mut dst[filled..]).await?;
        }
        Ok(())
    }
}

/// 扫同步字后读一条完整消息，返回消息长度（`cmd` 到 `sum8`）；`0` 表示长度非法、已丢弃。
async fn read_message<T: Read>(
    reader: &mut Reader,
    link: &mut T,
    msg: &mut [u8],
) -> Result<usize, T::Error> {
    let mut prev = 0u8;
    loop {
        let byte = reader.byte(link).await?;
        if prev == SYNC[0] && byte == SYNC[1] {
            break;
        }
        prev = byte;
    }

    reader.read_into(link, &mut msg[..3]).await?;
    let len = usize::from(u16::from_le_bytes([msg[1], msg[2]]));
    if 3 + len + 1 > msg.len() {
        return Ok(0);
    }

    reader.read_into(link, &mut msg[3..3 + len + 1]).await?;
    Ok(3 + len + 1)
}

/// 校验整条消息的字节和。
fn checksum_ok(msg: &[u8]) -> bool {
    let sum = msg[..msg.len() - 1]
        .iter()
        .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
    sum == msg[msg.len() - 1]
}

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

/// 解一块游程编码的 tile。
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

/// 画一批 tile，返回 `(画好的块数, 这批字节的哈希之和)`。
async fn draw_tiles(display: &mut Display, tiles: &[u8], count: usize) -> (u16, u32) {
    let Some(spans) = tile_spans(tiles, count) else {
        warn!("bad tile layout in batch of {}", count);
        return (0, 0);
    };

    let mut pixels = [0u16; TILE * TILE];
    let mut drawn = 0u16;
    let mut hash = 0u32;
    for (head, body, len) in spans {
        let data = &tiles[body..body + len];
        match tiles[head + 2] {
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

        let col = tiles[head] as u16;
        let row = tiles[head + 1] as u16;
        let x0 = col * TILE as u16;
        let y0 = row * TILE as u16;
        display
            .draw_raw_slice(x0, y0, x0 + TILE as u16 - 1, y0 + TILE as u16 - 1, &pixels)
            .expect("tile draw failed");
        hash = hash.wrapping_add(tile_hash(&tiles[head..body + len]));
        drawn += 1;
        // 每块都让出 CPU：UART 的 128 字节 FIFO 在 2 Mbaud 下只有 640 µs 余量，
        // 画一块的时长已经超过它，攒几块再让就会 Rx(FifoOverflowed) 丢整条消息
        yield_now().await;
    }
    (drawn, hash)
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

/// 板子侧的帧进度：当前帧号、本帧已经画了多少、数据哈希。
struct Progress {
    /// 当前帧号，0 表示还没开始。
    frame: u16,
    /// 本帧已应用的块数。
    tiles: u16,
    /// 本帧已应用数据的逐块哈希之和。
    hash: u32,
    /// 本帧第一批的时刻。
    start: Option<Instant>,
    /// 本帧从第一批到画完最后一批的毫秒数，上位机据此估板子能力。
    busy_ms: u16,
    /// 本帧已画完（LAST 已画）。
    done: bool,
    /// 帧率采样。
    rate: Rate,
    /// 累计块数，只为打日志。
    total: u32,
}

impl Progress {
    const fn new() -> Self {
        Self {
            frame: 0,
            tiles: 0,
            hash: 0,
            start: None,
            busy_ms: 0,
            done: false,
            rate: Rate::new(),
            total: 0,
        }
    }
}


/// 队列里一个待渲染的块：整条消息的槽 + 长度（`total == 0` 表示清屏）。
pub struct Block {
    buf: &'static mut [u8],
    total: usize,
}

/// 渲染进度快照：ACK 用它，可能落后一帧，只用于统计与上位机自校验。
#[derive(Clone, Copy)]
pub struct Snap {
    /// 本帧的 LAST 批已经画完，计数是最终值
    complete: bool,
    frame: u16,
    tiles: u16,
    hash: u32,
    busy_ms: u16,
    fps: u8,
}

impl Snap {
    const fn new() -> Self {
        Self {
            complete: false,
            frame: 0,
            tiles: 0,
            hash: 0,
            busy_ms: 0,
            fps: 0,
        }
    }

    /// 组一帧应答：`status | fps | busy(2) | frame(2) | tiles(2) | hash(4)`。
    fn report(&self, status: u8) -> [u8; REPLY_MAX] {
        let mut reply = [0u8; REPLY_MAX];
        reply[0..2].copy_from_slice(&SYNC);
        reply[2] = RSP_REPORT;
        reply[3..5].copy_from_slice(&REPORT_LEN.to_le_bytes());
        // 最高位告诉上位机：这一帧已经画完，tiles/hash 才是最终值
        reply[5] = status | if self.complete { ST_COMPLETE } else { 0 };
        reply[6] = self.fps;
        reply[7..9].copy_from_slice(&self.busy_ms.to_le_bytes());
        reply[9..11].copy_from_slice(&self.frame.to_le_bytes());
        reply[11..13].copy_from_slice(&self.tiles.to_le_bytes());
        reply[13..17].copy_from_slice(&self.hash.to_le_bytes());
        reply[17] = reply[2..17]
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        reply
    }
}

/// 把槽池铺进空闲表；`main` 在起任务前调用一次。
pub fn init_pool() {
    for chunk in POOL.init([0; MSG_MAX * POOL_BLOCKS]).chunks_mut(MSG_MAX) {
        let _ = FREE.try_send(chunk);
    }
}

/// 画一批属于当前帧的数据，顺带更新进度。
async fn apply(
    display: &mut Display,
    progress: &mut Progress,
    tiles: &[u8],
    count: usize,
    last: bool,
) {
    if progress.start.is_none() {
        progress.start = Some(Instant::now());
    }
    let (drawn, hash) = draw_tiles(display, tiles, count).await;
    progress.tiles = progress.tiles.saturating_add(drawn);
    progress.hash = progress.hash.wrapping_add(hash);
    progress.total += u32::from(drawn);

    if last {
        progress.done = true;
        if let Some(start) = progress.start.take() {
            // 至少算 1 ms：单批 1 块的帧真实耗时不足 1 ms，报 0 会让上位机以为是「没数据」
            progress.busy_ms = start.elapsed().as_millis().clamp(1, u64::from(u16::MAX)) as u16;
        }
        progress.rate.mark();
    }
    if progress.total >= LOG_EVERY {
        info!(
            "tiles {}, fps {}, frame {}",
            progress.total,
            progress.rate.fps(),
            progress.frame
        );
        progress.total = 0;
    }
}

/// 校验并画一个块，返回它是否是本帧的 LAST 块。
/// 块在进队列（或进「下一帧」缓存）前已由收帧任务校验过字节和与布局，这里不再重算
/// —— 1 块粒度下每块重算一遍会把每块渲染开销抬高一截。
async fn apply_block(display: &mut Display, progress: &mut Progress, block: &Block) -> bool {
    let msg = &block.buf[..block.total];
    if msg.len() < 3 + BATCH_HEAD + 1 {
        return false;
    }
    let payload = &msg[3..msg.len() - 1];
    let last = payload[2] & LAST_BATCH != 0;
    let count = usize::from(payload[3]);
    let tiles = &payload[BATCH_HEAD..];
    apply(display, progress, tiles, count, last).await;
    last
}

/// 从块缓冲里取帧号（载荷首字段）。
fn frame_of(block: &Block) -> u16 {
    u16::from_le_bytes([block.buf[3], block.buf[4]])
}

/// 渲染任务：单一所有者按到达顺序画块，**不缓存也不丢**。
///
/// 每块 tile 按坐标幂等绘制，所以就算上一帧的块迟到也照画——那正是该 tile 的最新
/// 内容；帧号只在变大时用来重置本帧计数与忙碌统计。不攒「下一帧」缓存，就不会
/// 因为缓存装不下一整帧而被迫推进、把块丢掉（那正是花屏的来源）。
pub async fn render(display: Display) -> ! {
    let mut display = display;
    let mut progress = Progress::new();
    loop {
        let block = QUEUE.receive().await;
        if block.total == 0 {
            display
                .clear(embedded_graphics::pixelcolor::Rgb565::BLACK)
                .ok();
        } else {
            let frame = frame_of(&block);
            if frame > progress.frame {
                progress.frame = frame;
                progress.tiles = 0;
                progress.hash = 0;
                progress.done = false;
                progress.start = None;
            }
            apply_block(&mut display, &mut progress, &block).await;
        }
        let _ = FREE.send(block.buf).await;

        {
            let mut snap = SNAP.lock().await;
            *snap = Snap {
                complete: progress.done,
                frame: progress.frame,
                tiles: progress.tiles,
                hash: progress.hash,
                busy_ms: progress.busy_ms,
                fps: progress.rate.fps(),
            };
        }
    }
}

/// 把应答写回链路。
async fn reply<T: Write>(link: &mut T, report: &[u8])
where
    T::Error: Format,
{
    if let Err(error) = link.write_all(report).await {
        warn!("reply write failed: {:?}", error);
    }
}

/// 一条链路的收帧循环：常备一块读缓冲，读完整条消息入队，再从槽池补到下一块——
/// 补不到就是渲染满了，这里停下等，链路背压全靠这一点（USB 有流控字节顶在缓冲里，
/// UART 没有、溢出丢掉，流式数据可接受）。数据命令不回应答，控制命令一问一答。
pub async fn serve<T>(link: &mut T) -> !
where
    T: Read + Write,
    T::Error: Format,
{
    let mut reader = Reader::new();
    let mut buf = FREE.receive().await;
    loop {
        let total = match read_message(&mut reader, link, &mut buf).await {
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

        let cmd = buf[0];
        if cmd == CMD_TILES {
            // 数据命令不回应答：流式数据丢了就丢了，上位机只管发
            if !checksum_ok(&buf[..total]) || total < 3 + BATCH_HEAD + 1 {
                warn!("bad tile message, dropped");
                continue;
            }
            let _ = QUEUE.send(Block { buf, total }).await;
            buf = FREE.receive().await;
            continue;
        }
        if cmd == CMD_CLEAR {
            // 清屏交给渲染任务，避免两个任务同时动屏
            let _ = QUEUE.send(Block { buf, total: 0 }).await;
            buf = FREE.receive().await;
            let snap = *SNAP.lock().await;
            reply(link, &snap.report(ST_ACK)).await;
            continue;
        }

        let status = match cmd {
            CMD_PING => ST_PONG,
            CMD_RESET => {
                *SNAP.lock().await = Snap::new();
                ST_ACK
            }
            other => {
                warn!("unknown command {}, dropped", other);
                ST_NAK
            }
        };
        let snap = *SNAP.lock().await;
        reply(link, &snap.report(status)).await;
    }
}
