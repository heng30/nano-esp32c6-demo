//! 渲染任务：单一所有者画屏，顺带维护帧进度、忙碌毫秒与帧率。

use defmt::info;
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
use esp_hal::time::Instant;

use super::pool::{Block, FREE, QUEUE, SNAP};
use super::protocol::{
    BATCH_HEAD, IDLE_MS, LAST_BATCH, LOG_EVERY, MAX_FPS, REPLY_MAX, REPORT_LEN, RSP_REPORT,
    ST_COMPLETE, SYNC,
};
use super::tile::draw_tiles;
use crate::display::Display;

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
    /// 本帧累计的纯绘制耗时（微秒），不含等链路的时间。
    draw_us: u64,
    /// 本帧画完所有块的绘制毫秒数，上位机据此估板子的绘制能力。
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
            draw_us: 0,
            busy_ms: 0,
            done: false,
            rate: Rate::new(),
            total: 0,
        }
    }
}

/// 渲染进度快照：ACK 用它，可能落后一帧，只用于统计与上位机自校验。
#[derive(Clone, Copy)]
pub(super) struct Snap {
    /// 本帧的 LAST 批已经画完，计数是最终值
    complete: bool,
    frame: u16,
    tiles: u16,
    hash: u32,
    busy_ms: u16,
    fps: u8,
}

impl Snap {
    pub(super) const fn new() -> Self {
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
    pub(super) fn report(&self, status: u8) -> [u8; REPLY_MAX] {
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

/// 画一批属于当前帧的数据，顺带更新进度。
async fn apply(
    display: &mut Display,
    progress: &mut Progress,
    tiles: &[u8],
    count: usize,
    last: bool,
) {
    let (drawn, hash, draw_us) = draw_tiles(display, tiles, count).await;
    progress.draw_us += draw_us;
    progress.tiles = progress.tiles.saturating_add(drawn);
    progress.hash = progress.hash.wrapping_add(hash);
    progress.total += u32::from(drawn);

    if last {
        progress.done = true;
        // 只算纯绘制：把等链路的时间也计入会拖低上位机算出的目标帧率
        progress.busy_ms = progress
            .draw_us
            .div_ceil(1000)
            .clamp(1, u64::from(u16::MAX)) as u16;
        progress.draw_us = 0;
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
/// 块在进队列前已由收帧任务校验过字节和与布局，这里不再重算
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
            display.clear(Rgb565::BLACK).ok();
        } else {
            let frame = frame_of(&block);
            if frame > progress.frame {
                progress.frame = frame;
                progress.tiles = 0;
                progress.hash = 0;
                progress.done = false;
                progress.draw_us = 0;
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
