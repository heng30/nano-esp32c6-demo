//! 一条链路的收帧循环：数据入队、控制命令一问一答。

use core::sync::atomic::Ordering;

use defmt::{Format, info, warn};
use embassy_futures::select::{Either, select};
use embassy_time::Timer;
use embedded_io_async::{Read, Write};

use super::pool::{Block, FREE, QUEUE, SCALE, SNAP};
use super::protocol::{
    ANNOUNCE_MS, BATCH_HEAD, CMD_CLEAR, CMD_GEO_ACK, CMD_PING, CMD_RESET, CMD_SCALE, CMD_TILES,
    Fault, Reader, ST_ACK, ST_NAK, ST_PONG, TILE, checksum_ok, geometry_report, read_message,
    reply,
};
use super::render::Snap;

/// 一条链路的收帧循环：常备一块读缓冲，读完整条消息入队，再从槽池补到下一块——
/// 补不到就是渲染满了，这里停下等，链路背压全靠这一点（USB 有流控字节顶在缓冲里，
/// UART 没有、溢出丢掉，流式数据可接受）。数据命令不回应答，控制命令一问一答。
///
/// 读失败或对端关流就返回，由调用方决定重连（TCP）还是继续（串口）。
pub async fn serve<T>(link: &mut T)
where
    T: Read + Write,
    T::Error: Format,
{
    let mut reader = Reader::new();
    let mut buf = FREE.receive().await;
    // 每条链路各自确认几何信息：串口与 WiFi 三条链路可能先后连上，共用标志会让后到的收不到
    let mut acked = false;
    loop {
        // 上位机确认收到几何信息之前，隔一段时间重报一次；它一开始推流就说明已经知道尺寸
        let read = if acked {
            read_message(&mut reader, link, &mut buf).await.map(Some)
        } else {
            reply(link, &geometry_report()).await;
            // 未确认阶段上位机不会发数据，所以超时打断读取不会丢半个消息
            match select(
                read_message(&mut reader, link, &mut buf),
                Timer::after_millis(ANNOUNCE_MS),
            )
            .await
            {
                Either::First(result) => result.map(Some),
                Either::Second(()) => Ok(None),
            }
        };
        let total = match read {
            Ok(Some(0)) => {
                warn!("bad length, message dropped");
                continue;
            }
            Ok(Some(total)) => total,
            Ok(None) => continue,
            Err(fault) => {
                match fault {
                    Fault::Eof => info!("link closed by host"),
                    Fault::Io(error) => warn!("read failed: {:?}", error),
                }
                // 手上一块还回池子，否则每断一次线就永久少一块
                let _ = FREE.try_send(buf);
                return;
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
            CMD_SCALE => {
                let value = if total >= 4 { buf[3] } else { 0 };
                if value >= 1 && value <= TILE as u8 && TILE as u8 % value == 0 {
                    SCALE.store(value, Ordering::Relaxed);
                    *SNAP.lock().await = Snap::new();
                    info!("scale set to {}", value);
                    ST_ACK
                } else {
                    warn!("bad scale {}", value);
                    ST_NAK
                }
            }
            CMD_GEO_ACK => {
                acked = true;
                info!("geometry acked by host");
                ST_ACK
            }
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
