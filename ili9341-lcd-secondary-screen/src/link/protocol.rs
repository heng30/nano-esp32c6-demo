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
//! frame(2) | flags(1) | count(1) | batch(1) | total(1) | tile*
//! ```
//!
//! - `frame`：帧号，板子靠它丢过期数据、认出「下一帧」
//! - `flags` 最低位 = 本帧最后一批
//! - `batch` / `total`：本批在本帧里的序号与总批数
//!
//! 应答 `RSP_REPORT` 的载荷：`status(1) | fps(1) | busy_ms(2) | frame(2) | tiles(2) | hash(4)`。
//! `tiles` 是本帧已应用的块数，`hash` 是本帧已应用数据的逐块哈希之和——上位机拿它自校验，
//! 不看屏幕就知道板子有没有收全。上位机实现见 `secondary-screen-desktop/src/link.rs`，两边常量必须一起改。

use core::sync::atomic::Ordering;

use defmt::{Format, warn};
use embedded_io_async::{Read, Write};

use super::pool::SCALE;

/// 一个 tile 的边长（像素）。
pub(super) const TILE: usize = 16;
/// 屏的分辨率：上位机连上后由板子上报。
pub(super) const PANEL_W: u16 = 320;
pub(super) const PANEL_H: u16 = 240;
/// 默认采样倍数（1 表示不放大）：改成 2/4 可把像素数据降到 1/4、1/16，画质相应变粗。
/// 上位机可用 `0x05` 改写运行期的值。
pub(super) const SCALE_DEFAULT: u8 = 1;
/// 上位机确认几何信息之前的重发间隔。
pub(super) const ANNOUNCE_MS: u64 = 500;
/// 一条消息最多几块 tile：上位机按帧自适应（小帧 1 块、大帧最多 32 块）。
pub(super) const MAX_TILES: usize = 32;
/// 一块 tile 的像素字节数。
pub(super) const RAW_LEN: usize = TILE * TILE * 2;
/// 一块 tile 的编码：原样。
pub(super) const KIND_RAW: u8 = 0;
/// 一块 tile 的编码：游程（最高位 1 = 重复 n 次同色，否则 n 个像素原样，n ≤ 128）。
pub(super) const KIND_RLE: u8 = 1;
/// 一块 tile 的编码：4 色调色板 + 2 bpp 索引。
pub(super) const KIND_PAL2: u8 = 2;
/// 一块 tile 的编码：16 色调色板 + 4 bpp 索引。
pub(super) const KIND_PAL4: u8 = 3;
/// 一块 tile 的头：列、行、编码、载荷长度（u16）。
pub(super) const TILE_HEAD: usize = 5;
/// 数据载荷头：帧号、标志、本批块数、批序号、批总数。
pub(super) const BATCH_HEAD: usize = 6;
/// `CMD_TILES` 载荷的最大长度：批头 + 一批 tile。
const PAYLOAD_MAX: usize = BATCH_HEAD + MAX_TILES * (TILE_HEAD + RAW_LEN);
/// 一条消息的最大字节数（不含同步字）：`cmd + len + payload + sum`。
pub(super) const MSG_MAX: usize = 3 + PAYLOAD_MAX + 1;
/// 消息头的同步字。
pub(super) const SYNC: [u8; 2] = [0xC6, 0xD5];

/// 数据命令：一批 tile。
pub(super) const CMD_TILES: u8 = 0x01;
/// 控制命令：查询。
pub(super) const CMD_PING: u8 = 0x02;
/// 控制命令：清屏。
pub(super) const CMD_CLEAR: u8 = 0x03;
/// 控制命令：重置板子内部状态（帧号、进度、缓存、帧率采样），不动屏幕。
pub(super) const CMD_RESET: u8 = 0x04;
/// 控制命令：上位机确认收到几何信息。
pub(super) const CMD_GEO_ACK: u8 = 0x06;
/// 控制命令：改写采样倍数（载荷 1 字节，必须是 TILE 的约数）。
pub(super) const CMD_SCALE: u8 = 0x05;

/// 板子的控制应答。
pub(super) const RSP_REPORT: u8 = 0x81;
/// 板子上报几何信息（未确认前持续重发）。
pub(super) const RSP_GEOMETRY: u8 = 0x85;
/// 应答状态：收到并处理完。
pub(super) const ST_ACK: u8 = 0;
/// 应答状态：校验或命令错，上位机应重发。
pub(super) const ST_NAK: u8 = 1;
/// 应答状态：对 `CMD_PING` 的回应。
pub(super) const ST_PONG: u8 = 2;
/// 应答载荷长度：状态 + 帧率 + 忙碌毫秒 + 帧号 + 本帧块数 + 本帧哈希。
pub(super) const REPORT_LEN: u16 = 12;
/// 一条应答的总字节数：同步 + 命令 + 长度 + 载荷 + 校验。
pub(super) const REPLY_MAX: usize = 2 + 3 + REPORT_LEN as usize + 1;

/// 批标志：本帧最后一批。
pub(super) const LAST_BATCH: u8 = 0x01;
/// 应答状态的最高位：本帧已画完（计数是最终值）。
pub(super) const ST_COMPLETE: u8 = 0x80;
/// 上报帧率的上限，与上位机一致。
pub(super) const MAX_FPS: u8 = 30;
/// 超过这么久没收到整帧就丢掉帧率样本，免得安静期把间隔算进下一帧。
pub(super) const IDLE_MS: u64 = 2000;
/// 每收多少 tile 打一条日志。
pub(super) const LOG_EVERY: u32 = 2000;

/// 读链路中断的两类原因：`Io` 是驱动报错，`Eof` 是对端关了连接（只有 TCP 会这样）。
pub(super) enum Fault<E> {
    /// 驱动返回的错误。
    Io(E),
    /// 读到流结束。
    Eof,
}

/// 读一批字节；`Ok(0)` 在串口上不会出现，出现就是 TCP 对端关流。
async fn fill<T: Read>(link: &mut T, dst: &mut [u8]) -> Result<usize, Fault<T::Error>> {
    match link.read(dst).await {
        Ok(0) => Err(Fault::Eof),
        Ok(n) => Ok(n),
        Err(error) => Err(Fault::Io(error)),
    }
}

/// 从链路搬字节的小缓冲：SYNC 扫描逐字节，载荷整块拷。
/// 给到 8 KB：TCP 每次唤醒拿到多少字节决定吞吐（实测一次唤醒约 1 KB 就换一次 2 ms 等待），
/// 缓冲大一点才能一次排空驱动里排队的段。
pub(super) struct Reader {
    buf: [u8; 8192],
    pos: usize,
    len: usize,
}

impl Reader {
    pub(super) const fn new() -> Self {
        Self {
            buf: [0; 8192],
            pos: 0,
            len: 0,
        }
    }

    async fn byte<T: Read>(&mut self, link: &mut T) -> Result<u8, Fault<T::Error>> {
        if self.pos == self.len {
            self.len = fill(link, &mut self.buf).await?;
            self.pos = 0;
        }
        let byte = self.buf[self.pos];
        self.pos += 1;
        Ok(byte)
    }

    /// 拷满 `dst`：先吃缓冲里剩下的，不够再直接从链路读到目标缓冲。
    async fn read_into<T: Read>(
        &mut self,
        link: &mut T,
        dst: &mut [u8],
    ) -> Result<(), Fault<T::Error>> {
        let rest = self.len - self.pos;
        let n = rest.min(dst.len());
        dst[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;

        let mut filled = n;
        while filled < dst.len() {
            filled += fill(link, &mut dst[filled..]).await?;
        }
        Ok(())
    }
}

/// 扫同步字后读一条完整消息，返回消息长度（`cmd` 到 `sum8`）；`0` 表示长度非法、已丢弃。
pub(super) async fn read_message<T: Read>(
    reader: &mut Reader,
    link: &mut T,
    msg: &mut [u8],
) -> Result<usize, Fault<T::Error>> {
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
pub(super) fn checksum_ok(msg: &[u8]) -> bool {
    let sum = msg[..msg.len() - 1]
        .iter()
        .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
    sum == msg[msg.len() - 1]
}

/// 几何信息上报：`RSP_GEOMETRY | len=5 | 宽(2) | 高(2) | 倍数(1) | sum`。
pub(super) fn geometry_report() -> [u8; 11] {
    let mut reply = [0u8; 11];
    reply[0..2].copy_from_slice(&SYNC);
    reply[2] = RSP_GEOMETRY;
    reply[3..5].copy_from_slice(&5u16.to_le_bytes());
    reply[5..7].copy_from_slice(&PANEL_W.to_le_bytes());
    reply[7..9].copy_from_slice(&PANEL_H.to_le_bytes());
    reply[9] = SCALE.load(Ordering::Relaxed);
    reply[10] = reply[2..10]
        .iter()
        .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
    reply
}

/// 把应答写回链路。
pub(super) async fn reply<T: Write>(link: &mut T, report: &[u8])
where
    T::Error: Format,
{
    if let Err(error) = link.write_all(report).await {
        warn!("reply write failed: {:?}", error);
    }
}
