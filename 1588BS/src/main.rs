#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use defmt::info;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::{
    clock::CpuClock,
    gpio::{Level, Output, OutputConfig},
    timer::timg::TimerGroup,
};
use panic_rtt_target as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// 单行保持时间；8 行一圈 8ms，帧率约 125Hz，高于人眼融合阈值。
const ROW_TIME: Duration = Duration::from_millis(1);

/// 每帧播放时长。
const FRAME_TIME: Duration = Duration::from_millis(1000);

/// 循环播放的帧，每字节一行（下标 0..7 = ROW1..8），bit7 是最左列（COL1）。
const FRAMES: [[u8; 8]; 4] = [
    // 全亮
    [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
    // 棋盘，方便查行/列映射是否接反
    [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55],
    // 心形
    [
        0b00000000, 0b01100110, 0b11111111, 0b11111111, 0b01111110, 0b00111100, 0b00011000,
        0b00000000,
    ],
    // 叉号
    [
        0b10000001, 0b01000010, 0b00100100, 0b00011000, 0b00011000, 0b00100100, 0b01000010,
        0b10000001,
    ],
];

/// 1588BS 是共阳点阵：行是阳极（拉高选中），列是阴极（拉低点亮）。
struct Matrix {
    /// 行阳极 ROW1..8，高电平选中。
    row: [Output<'static>; 8],
    /// 列阴极 COL1..8，低电平点亮。
    col: [Output<'static>; 8],
}

impl Matrix {
    /// 上电先全灭：行拉低、列拉高，避免引脚复位后的随机电平让整片微亮。
    fn new(mut row: [Output<'static>; 8], mut col: [Output<'static>; 8]) -> Self {
        for r in row.iter_mut() {
            r.set_low();
        }
        for c in col.iter_mut() {
            c.set_high();
        }
        Self { row, col }
    }

    /// 用逐行扫描把 `fb` 显示 `ms` 毫秒。
    ///
    /// 同一时刻只有一行阳极带电，只有这一行的列电平有意义；每行只亮 1/8 时间，
    /// 亮度是满亮的 1/8。
    async fn show(&mut self, fb: &[u8; 8], ms: u64) {
        let rounds = ms * 1000 / (ROW_TIME.as_micros() * 8);
        for _ in 0..rounds {
            for (r, &mask) in fb.iter().enumerate() {
                // 上一行已经拉低，此刻改列电平不会点亮任何点，不必先消隐
                for (i, c) in self.col.iter_mut().enumerate() {
                    if mask & (0x80 >> i) != 0 {
                        c.set_low();
                    } else {
                        c.set_high();
                    }
                }
                self.row[r].set_high();
                Timer::after(ROW_TIME).await;
                self.row[r].set_low();
            }
        }
    }
}

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    // 行 ROW1..8 = GPIO0..7，列 COL1..8 = GPIO10/11/18..23
    // （避开 USB 12/13、BOOT 9、串口 16/17、strapping 15、板载 RGB 8）
    let mut matrix = Matrix::new(
        [
            Output::new(peripherals.GPIO0, Level::Low, OutputConfig::default()),
            Output::new(peripherals.GPIO1, Level::Low, OutputConfig::default()),
            Output::new(peripherals.GPIO2, Level::Low, OutputConfig::default()),
            Output::new(peripherals.GPIO3, Level::Low, OutputConfig::default()),
            Output::new(peripherals.GPIO4, Level::Low, OutputConfig::default()),
            Output::new(peripherals.GPIO5, Level::Low, OutputConfig::default()),
            Output::new(peripherals.GPIO6, Level::Low, OutputConfig::default()),
            Output::new(peripherals.GPIO7, Level::Low, OutputConfig::default()),
        ],
        [
            Output::new(peripherals.GPIO10, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO11, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO18, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO19, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO20, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO21, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO22, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO23, Level::High, OutputConfig::default()),
        ],
    );

    info!("1588BS ready");

    let mut i = 0usize;
    loop {
        info!("frame {}", i % FRAMES.len());
        matrix
            .show(&FRAMES[i % FRAMES.len()], FRAME_TIME.as_millis())
            .await;
        i += 1;
    }
}
