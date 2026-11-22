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

/// 段码表，bit0..7 依次对应 A B C D E F G DP，置位表示该段点亮（与极性无关）。
///
/// DP 位恒为 0，小数点由 `DP_BIT` 单独叠加。
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

/// DP（小数点）在段码里的 bit 位置。
const DP_BIT: u8 = 1 << 7;

/// 点亮一段时 GPIO 的电平：这只数码管是共阴，公共端接 GND，段选拉高点亮。
const SEG_ON: Level = Level::High;

/// 熄灭一段时 GPIO 的电平，与 `SEG_ON` 相反。
const SEG_OFF: Level = Level::Low;

/// 每位数字停留多久，也就是刷新间隔。
const STEP: Duration = Duration::from_secs(1);

/// 单个数码管的 A..G 七段加小数点，下标即段码表里的 bit 位置。
///
/// 公共端不走 GPIO，直接接 GND：整只管子亮起来时公共端要过几十 mA，超过单脚灌入能力。
struct Segments {
    /// 段选 A B C D E F G DP。
    seg: [Output<'static>; 8],
}

impl Segments {
    /// 上电先把所有段熄掉，免得引脚复位后的随机电平让某些段微亮。
    fn new(mut seg: [Output<'static>; 8]) -> Self {
        for s in seg.iter_mut() {
            s.set_level(SEG_OFF);
        }
        Self { seg }
    }

    /// 按段码位图直接驱动八根段线，bit 置位的那一段点亮。
    fn show_mask(&mut self, mask: u8) {
        for (bit, s) in self.seg.iter_mut().enumerate() {
            s.set_level(if mask & (1 << bit) != 0 {
                SEG_ON
            } else {
                SEG_OFF
            });
        }
    }

    /// 按段码表点亮 `digit`（0–9），`dp` 为真时同时点亮小数点。
    fn show(&mut self, digit: u8, dp: bool) {
        let mask = FONT[digit as usize % FONT.len()] | if dp { DP_BIT } else { 0 };
        self.show_mask(mask);
    }
}

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let mut display = Segments::new([
        Output::new(peripherals.GPIO0, SEG_OFF, OutputConfig::default()),
        Output::new(peripherals.GPIO1, SEG_OFF, OutputConfig::default()),
        Output::new(peripherals.GPIO2, SEG_OFF, OutputConfig::default()),
        Output::new(peripherals.GPIO3, SEG_OFF, OutputConfig::default()),
        Output::new(peripherals.GPIO4, SEG_OFF, OutputConfig::default()),
        Output::new(peripherals.GPIO5, SEG_OFF, OutputConfig::default()),
        Output::new(peripherals.GPIO6, SEG_OFF, OutputConfig::default()),
        Output::new(peripherals.GPIO7, SEG_OFF, OutputConfig::default()),
    ]);

    info!("baike ready");

    // 0–9 每秒加一，小数点只在轮首的 0 上亮一个 STEP
    let mut digit = 0u8;
    loop {
        display.show(digit, digit == 0);
        info!("digit {}", digit);
        digit = (digit + 1) % 10;
        Timer::after(STEP).await;
    }
}
