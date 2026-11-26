//! 消息槽池、待渲染队列、渲染进度快照与当前采样倍数。

use core::sync::atomic::AtomicU8;

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, mutex::Mutex};
use static_cell::StaticCell;

use super::protocol::{MSG_MAX, SCALE_DEFAULT};
use super::render::Snap;

/// 槽池：每条消息一块（最大约 4 KB）；13 块刚好装得下一整帧（300 块 / 32 块一条 = 10 条）
/// 加上两条链路各在手的一块与渲染在手的一块。
/// 收帧任务补不到块就不读下一条：链路背压全靠这一点（USB 有流控字节顶在缓冲里，
/// UART 没有会溢出丢掉，流式数据可接受）。
const POOL_BLOCKS: usize = 13;

static POOL: StaticCell<[u8; MSG_MAX * POOL_BLOCKS]> = StaticCell::new();
/// 空闲槽表。
pub(super) static FREE: Channel<CriticalSectionRawMutex, &'static mut [u8], POOL_BLOCKS> =
    Channel::new();
/// 待渲染队列：收到块就投进来，ACK 等渲染腾出下一块槽再回。
pub(super) static QUEUE: Channel<CriticalSectionRawMutex, Block, POOL_BLOCKS> = Channel::new();
/// 渲染进度快照，收发任务回 ACK 时读它。
pub(super) static SNAP: Mutex<CriticalSectionRawMutex, Snap> = Mutex::new(Snap::new());
/// 当前采样倍数：上位机不传 `--scale` 就沿用这里的默认值。
pub(super) static SCALE: AtomicU8 = AtomicU8::new(SCALE_DEFAULT);

/// 队列里一个待渲染的块：整条消息的槽 + 长度（`total == 0` 表示清屏）。
pub(super) struct Block {
    pub(super) buf: &'static mut [u8],
    pub(super) total: usize,
}

/// 把槽池铺进空闲表；`main` 在起任务前调用一次。
pub fn init_pool() {
    for chunk in POOL.init([0; MSG_MAX * POOL_BLOCKS]).chunks_mut(MSG_MAX) {
        let _ = FREE.try_send(chunk);
    }
}
