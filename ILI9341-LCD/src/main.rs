#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use defmt::info;
use embassy_executor::Spawner;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    timer::timg::TimerGroup,
};
use panic_rtt_target as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// 裸 8080 总线：16 根数据线 + RS(DC) + WR。CS 接地，RD 拉高。
struct Bus {
    data: [Output<'static>; 16],
    dc: Output<'static>,
    wr: Output<'static>,
}

impl Bus {
    fn new(data: [Output<'static>; 16], dc: Output<'static>, wr: Output<'static>) -> Self {
        Self { data, dc, wr }
    }

    fn set_bus(&mut self, value: u16) {
        for (bit, pin) in self.data.iter_mut().enumerate() {
            if value & (1 << bit) != 0 {
                pin.set_high();
            } else {
                pin.set_low();
            }
        }
    }

    fn strobe(&mut self) {
        self.wr.set_low();
        self.wr.set_high();
    }

    fn write(&mut self, value: u16, is_data: bool) {
        if is_data {
            self.dc.set_high();
        } else {
            self.dc.set_low();
        }
        self.set_bus(value);
        self.strobe();
    }

    fn cmd(&mut self, cmd: u8) {
        self.write(u16::from(cmd), false);
    }

    fn data(&mut self, value: u8) {
        self.write(u16::from(value), true);
    }

    fn pixel(&mut self, color: u16) {
        self.set_bus(color);
        self.strobe();
    }
}

fn init(bus: &mut Bus, delay: &Delay) {
    // 软件复位，等 120ms
    bus.cmd(0x01);
    delay.delay_millis(120);

    // 退出睡眠
    bus.cmd(0x11);
    delay.delay_millis(120);

    // 像素格式 = 16 位 RGB565
    bus.cmd(0x3A);
    bus.data(0x55);

    // 内存访问控制：横屏、RGB 顺序
    bus.cmd(0x36);
    bus.data(0x48);

    // 显示开
    bus.cmd(0x29);
    delay.delay_millis(50);

    info!("init sequence sent");
}

fn fill(bus: &mut Bus, color: u16) {
    // 列地址 0..239
    bus.cmd(0x2A);
    bus.data(0x00);
    bus.data(0x00);
    bus.data(0x00);
    bus.data(0xEF);

    // 行地址 0..319
    bus.cmd(0x2B);
    bus.data(0x00);
    bus.data(0x00);
    bus.data(0x01);
    bus.data(0x3F);

    // 写 GRAM
    bus.cmd(0x2C);
    for _ in 0..(240 * 320) {
        bus.pixel(color);
    }
}

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let delay = Delay::new();

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

    let mut bus = Bus::new(
        data,
        Output::new(peripherals.GPIO9, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO23, Level::High, OutputConfig::default()),
    );

    init(&mut bus, &delay);

    // 逐位扫描：每次只点亮一根数据线，看屏上出现什么。
    // 预期 bit0-4 = 蓝(bit4 最亮)，bit5-10 = 绿(bit10 最亮)，bit11-15 = 红(bit15 最亮)。
    let bits = [
        0x0001u16, 0x0002, 0x0004, 0x0008, 0x0010, 0x0020, 0x0040, 0x0080, 0x0100, 0x0200,
        0x0400, 0x0800, 0x1000, 0x2000, 0x4000, 0x8000,
    ];

    loop {
        for (bit, value) in bits.iter().enumerate() {
            info!("bit {} = 0x{:04X}", bit, value);
            fill(&mut bus, *value);
            delay.delay_millis(1200);
        }
    }
}
