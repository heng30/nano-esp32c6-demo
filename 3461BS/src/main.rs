#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use defmt::info;
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    clock::CpuClock,
    gpio::{Level, Output, OutputConfig},
    rmt::Rmt,
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_hal_smartled::{RmtSmartLeds, buffer_size, color_order};
use panic_rtt_target as _;
use smart_leds_trait::{RGB8, SmartLedsWrite};

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

/// DP（小数点）在段码里的 bit 位置。
const DP_BIT: u8 = 1 << 7;

/// 板载 WS2812B 上电自检颜色。
const LED_BOOT: RGB8 = RGB8 {
    r: 0x20,
    g: 0x20,
    b: 0x20,
};

/// 3461BS 是共阳数码管：位选为阳极拉高选中，段选为阴极拉低点亮。
struct Display {
    /// 段选 A B C D E F G DP，低电平点亮；四位共用这一组，所以只能分时扫描。
    seg: [Output<'static>; 8],
    /// 位选 DIG1..4，高电平选中；下标 0 是最左边一位。
    dig: [Output<'static>; 4],
}

impl Display {
    /// 上电后先把所有段和位都关掉，避免引脚复位后的随机电平让某些段微亮。
    fn new(mut seg: [Output<'static>; 8], mut dig: [Output<'static>; 4]) -> Self {
        for s in seg.iter_mut() {
            s.set_high();
        }
        for d in dig.iter_mut() {
            d.set_low();
        }
        Self { seg, dig }
    }

    /// 扫描一遍四位数字，每位保持 2ms；`dp` 是点亮小数点的位下标（0 为最左）。
    ///
    /// 一圈 8ms，帧率约 125Hz，高于人眼融合阈值，看上去四位同时亮；
    /// 代价是每位只亮 1/4 时间，亮度也是满亮的 1/4。
    async fn sweep(&mut self, value: u16, dp: usize) {
        // 拆成四位，digits[0] 放最高位，好和 dig[0]（最左）对齐
        let mut digits = [0u8; 4];
        let mut v = value % 10000;
        for d in digits.iter_mut().rev() {
            *d = (v % 10) as u8;
            v /= 10;
        }

        for (i, n) in digits.iter().enumerate() {
            // 先熄灭所有段再换位，否则上一位的段码会在下一位闪一下残影
            for s in self.seg.iter_mut() {
                s.set_high();
            }
            // 只留第 i 位的阳极，其余三位没有电压差，真的不发光（不是被盖住）
            for (j, d) in self.dig.iter_mut().enumerate() {
                if j == i {
                    d.set_high();
                } else {
                    d.set_low();
                }
            }
            // 选定人选之后才灌内容：段线只有一组，此刻它属于第 i 位
            let mask = FONT[*n as usize] | if i == dp { DP_BIT } else { 0 };
            for (bit, s) in self.seg.iter_mut().enumerate() {
                if mask & (1 << bit) != 0 {
                    s.set_low();
                } else {
                    s.set_high();
                }
            }
            Timer::after(Duration::from_millis(2)).await;
        }

        // 收尾熄灭：否则 DIG4 位选仍为高，返回后到下一次 sweep 之间第四位会一直亮
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

    // 板载 WS2812B（GPIO8）不掉电就保持上次写入的颜色：上电亮 1 秒当作自检，再熄灭
    let freq = Rate::from_mhz(80);
    let rmt = Rmt::new(peripherals.RMT, freq).expect("RMT init failed");
    let mut led = RmtSmartLeds::<{ buffer_size::<RGB8>(1) }, _, RGB8, color_order::Grb>::new(
        esp_hal_smartled::WS2812B_TIMING,
        rmt.channel0,
        peripherals.GPIO8,
        freq,
    )
    .expect("LED init failed");
    led.write([LED_BOOT].into_iter()).expect("LED write failed");
    Timer::after(Duration::from_secs(1)).await;
    led.write([RGB8::default()].into_iter())
        .expect("LED clear failed");

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

    // 用实测时间驱动计数，不靠扫描次数累加，长时间跑不会漂
    let start = Instant::now();
    let mut last_secs = u64::MAX;

    loop {
        let ms = start.elapsed().as_millis();
        let value = ((ms / 100) % 10000) as u16;
        let dp = ((ms / 10_000) % 4) as usize;
        let secs = ms / 1000;
        if secs != last_secs {
            last_secs = secs;
            info!("{} dp:{}", value, dp);
        }
        display.sweep(value, dp).await;
    }
}
