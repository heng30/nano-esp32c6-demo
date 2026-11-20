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

/// 段码表，bit0..7 依次对应 A B C D E F G DP，置位表示该段点亮。
const FONT: [u8; 10] = [
    0x3F, // 0
    0x06, // 1
    0x5B, // 2
    0x4F, // 3
    0x66, // 4
    0x6D, // 5
    0x7D, // 6
    0x07, // 7
    0x7F, // 8
    0x6F, // 9
];

/// 3461BS 是共阳数码管：位选为阳极拉高选中，段选为阴极拉低点亮。
struct Display {
    seg: [Output<'static>; 8],
    dig: [Output<'static>; 4],
}

impl Display {
    fn new(mut seg: [Output<'static>; 8], mut dig: [Output<'static>; 4]) -> Self {
        for s in seg.iter_mut() {
            s.set_high();
        }
        for d in dig.iter_mut() {
            d.set_low();
        }
        Self { seg, dig }
    }

    /// 扫描一遍四位数字，每位保持 2ms。
    async fn sweep(&mut self, value: u16) {
        let mut digits = [0u8; 4];
        let mut v = value % 10000;
        for d in digits.iter_mut().rev() {
            *d = (v % 10) as u8;
            v /= 10;
        }

        for (i, n) in digits.iter().enumerate() {
            for s in self.seg.iter_mut() {
                s.set_high();
            }
            for (j, d) in self.dig.iter_mut().enumerate() {
                if j == i {
                    d.set_high();
                } else {
                    d.set_low();
                }
            }
            let mask = FONT[*n as usize];
            for (bit, s) in self.seg.iter_mut().enumerate() {
                if mask & (1 << bit) != 0 {
                    s.set_low();
                } else {
                    s.set_high();
                }
            }
            Timer::after(Duration::from_millis(2)).await;
        }

        for s in self.seg.iter_mut() {
            s.set_high();
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

    // 段选 A..DP = GPIO0..7，位选 DIG1..4 = GPIO10/11/18/19（避开 USB 12/13 与 strapping 8/9）
    let mut display = Display::new(
        [
            Output::new(peripherals.GPIO0, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO1, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO2, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO3, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO4, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO5, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO6, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO7, Level::High, OutputConfig::default()),
        ],
        [
            Output::new(peripherals.GPIO10, Level::Low, OutputConfig::default()),
            Output::new(peripherals.GPIO11, Level::Low, OutputConfig::default()),
            Output::new(peripherals.GPIO18, Level::Low, OutputConfig::default()),
            Output::new(peripherals.GPIO19, Level::Low, OutputConfig::default()),
        ],
    );

    info!("3461BS ready");

    let mut value: u16 = 0;
    let mut ticks: u32 = 0;

    loop {
        display.sweep(value).await;
        // 一次扫描约 8ms，攒够 125 次约 1 秒再进位
        ticks += 1;
        if ticks == 125 {
            ticks = 0;
            value = (value + 1) % 10000;
            info!("{}", value);
        }
    }
}
