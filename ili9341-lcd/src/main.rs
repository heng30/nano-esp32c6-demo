#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use defmt::info;
use display_interface::{DataFormat, DisplayError, WriteOnlyDataCommand};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    draw_target::DrawTarget,
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    timer::timg::TimerGroup,
};
use ili9341::{DisplaySize240x320, Ili9341, Orientation};
use panic_rtt_target as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// 一圈换一种底色，配色顺序即演示顺序。
const COLORS: [Rgb565; 3] = [Rgb565::RED, Rgb565::GREEN, Rgb565::BLUE];

/// 每圈停留时长，肉眼确认整屏填充完成。
const HOLD: Duration = Duration::from_secs(1);

/// 屏的 RST 与板子 EN 共复位，没有独立 GPIO。`ili9341` 初始化时会拉低再拉高一次，
/// 这里用空脚顶替，避免占用一个不存在的引脚。
struct NoReset;

impl embedded_hal::digital::ErrorType for NoReset {
    type Error = core::convert::Infallible;
}

impl embedded_hal::digital::OutputPin for NoReset {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// 8080-16 位并口：16 根数据线 + RS(DC) + WR。CS 接地、RD 接 3V3，只写不读。
struct Bus {
    /// D0..D15，下标即位序。
    data: [Output<'static>; 16],
    /// RS/D/CX：低=命令，高=数据。
    dc: Output<'static>,
    /// WR：上升沿锁存。
    wr: Output<'static>,
}

impl Bus {
    fn new(data: [Output<'static>; 16], dc: Output<'static>, wr: Output<'static>) -> Self {
        Self { data, dc, wr }
    }

    /// 一次并口写：摆好 16 位数据与 DC，再打一个 WR 脉冲。
    fn write(&mut self, value: u16, is_data: bool) {
        self.dc
            .set_level(if is_data { Level::High } else { Level::Low });
        for (bit, pin) in self.data.iter_mut().enumerate() {
            pin.set_level(if value & (1 << bit) != 0 {
                Level::High
            } else {
                Level::Low
            });
        }
        self.wr.set_low();
        self.wr.set_high();
    }
}

impl WriteOnlyDataCommand for Bus {
    fn send_commands(&mut self, cmd: DataFormat<'_>) -> Result<(), DisplayError> {
        let DataFormat::U8Iter(iter) = cmd else {
            return Err(DisplayError::DataFormatNotImplemented);
        };
        for byte in iter {
            self.write(u16::from(byte), false);
        }
        Ok(())
    }

    fn send_data(&mut self, buf: DataFormat<'_>) -> Result<(), DisplayError> {
        match buf {
            // 16 位并口下命令参数走 D[7:0]
            DataFormat::U8Iter(iter) => {
                for byte in iter {
                    self.write(u16::from(byte), true);
                }
            }
            // 像素一个写周期送完整 16 位 RGB565
            DataFormat::U16BEIter(iter) => {
                for pixel in iter {
                    self.write(pixel, true);
                }
            }
            _ => return Err(DisplayError::DataFormatNotImplemented),
        }
        Ok(())
    }
}

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let mut delay = Delay::new();

    let data = [
        Output::new(peripherals.GPIO0, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO1, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO2, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO3, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO4, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO5, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO6, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO7, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO8, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO10, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO11, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO18, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO19, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO20, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO21, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO22, Level::Low, OutputConfig::default()),
    ];

    let bus = Bus::new(
        data,
        Output::new(peripherals.GPIO15, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO23, Level::High, OutputConfig::default()),
    );

    let mut display = Ili9341::new(
        bus,
        NoReset,
        &mut delay,
        Orientation::Landscape,
        DisplaySize240x320,
    )
    .expect("ili9341 init failed");
    info!("ili9341 ready {}x{}", display.width(), display.height());

    let mut step = 0usize;
    loop {
        display
            .clear(COLORS[step % COLORS.len()])
            .expect("clear failed");
        Rectangle::new(Point::new(40, 30), Size::new(240, 180))
            .into_styled(PrimitiveStyle::with_fill(COLORS[(step + 1) % COLORS.len()]))
            .draw(&mut display)
            .expect("draw failed");
        info!("step {}", step);

        step += 1;
        Timer::after(HOLD).await;
    }
}
