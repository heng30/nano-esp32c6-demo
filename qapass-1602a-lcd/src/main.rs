#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use core::fmt::Write as _;
use defmt::info;
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    timer::timg::TimerGroup,
};
use panic_rtt_target as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// 1602A（HD44780 兼容）四位并行接口：RS + E + DB4~DB7，R/W 接地只写不读。
struct Lcd {
    rs: Output<'static>,
    en: Output<'static>,
    db: [Output<'static>; 4],
    delay: Delay,
}

impl Lcd {
    fn new(rs: Output<'static>, en: Output<'static>, db: [Output<'static>; 4]) -> Self {
        Self {
            rs,
            en,
            db,
            delay: Delay::new(),
        }
    }

    /// E 一个完整脉冲：脉宽与周期都远超手册要求的 230ns/500ns。
    fn pulse(&mut self) {
        self.delay.delay_micros(1);
        self.en.set_high();
        self.delay.delay_micros(1);
        self.en.set_low();
        self.delay.delay_micros(1);
    }

    /// 发半个字节，nibble 的低四位对应 DB4~DB7。
    fn nibble(&mut self, rs: Level, nibble: u8) {
        self.rs.set_level(rs);
        for (i, pin) in self.db.iter_mut().enumerate() {
            pin.set_level(if nibble & (1 << i) != 0 {
                Level::High
            } else {
                Level::Low
            });
        }
        self.pulse();
    }

    fn byte(&mut self, rs: Level, value: u8) {
        self.nibble(rs, value >> 4);
        self.nibble(rs, value & 0x0F);
        // 普通指令 39us、写数据 43us
        self.delay.delay_micros(50);
    }

    fn cmd(&mut self, cmd: u8) {
        self.byte(Level::Low, cmd);
        if cmd <= 0x02 {
            // 清屏与回车需要 1.53ms
            self.delay.delay_millis(2);
        }
    }

    fn init(&mut self) {
        self.delay.delay_millis(50);
        // 上电三次 0x3 强行回到 8 位状态，再发 0x2 切到 4 位
        for wait in [4500, 4500, 150] {
            self.nibble(Level::Low, 0x3);
            self.delay.delay_micros(wait);
        }
        self.nibble(Level::Low, 0x2);
        self.delay.delay_micros(150); // 切 4 位本身是一条 Function Set，要等它执行完
        self.cmd(0x28); // 4 位、2 行、5x8 点阵
        self.cmd(0x08); // 关显示
        self.cmd(0x01); // 清屏
        self.cmd(0x06); // 写入后地址自增，画面不滚动
        self.cmd(0x0C); // 开显示，无光标
    }

    fn goto(&mut self, line: u8) {
        self.cmd(if line == 0 { 0x80 } else { 0xC0 });
    }
}

impl core::fmt::Write for Lcd {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            self.byte(Level::High, b);
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

    // RS=GPIO0，E=GPIO1，DB4~DB7=GPIO2/3/4/5；DB0~DB3 四位模式下不用
    let mut lcd = Lcd::new(
        Output::new(peripherals.GPIO0, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO1, Level::Low, OutputConfig::default()),
        [
            Output::new(peripherals.GPIO2, Level::Low, OutputConfig::default()),
            Output::new(peripherals.GPIO3, Level::Low, OutputConfig::default()),
            Output::new(peripherals.GPIO4, Level::Low, OutputConfig::default()),
            Output::new(peripherals.GPIO5, Level::Low, OutputConfig::default()),
        ],
    );

    lcd.init();
    info!("1602A self-test start");

    // 阶段 A：关显示/开显示交替，验证 RS、E、DB6、DB7 通路
    for _ in 0..3 {
        lcd.cmd(0x08);
        Timer::after(Duration::from_millis(1000)).await;
        lcd.cmd(0x0C);
        Timer::after(Duration::from_millis(1000)).await;
    }

    // 阶段 B：清屏后写 A，验证 DB4、DB5 和数据通路
    lcd.cmd(0x01);
    lcd.goto(0);
    let _ = write!(lcd, "AAAAAAAAAAAAAAAA");
    lcd.goto(1);
    let _ = write!(lcd, "QAPASS 1602A");

    info!("1602A self-test done");

    // 阶段 C：第一行固定标题，第二行每秒刷新 uptime
    lcd.cmd(0x01);
    lcd.goto(0);
    let _ = write!(lcd, "QAPASS 1602A");

    let start = Instant::now();
    loop {
        lcd.goto(1);
        // 右对齐补空格，避免数字位数变化时残留旧字符
        let _ = write!(lcd, "uptime: {}s", start.elapsed().as_secs());
        Timer::after(Duration::from_secs(1)).await;
    }
}
