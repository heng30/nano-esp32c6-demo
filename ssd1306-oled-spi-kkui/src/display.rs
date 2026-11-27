//! SSD1306 128×64 OLED 的 SPI 屏：初始化、地址窗口与整帧递交都交给 `ssd1306` crate。
//!
//! kk-ui 的帧缓冲与 SSD1306 的 GDDRAM 布局一致（每字节是同列连续 8 行、bit0 在最上），
//! 所以整帧就是一次 `draw`：horizontal 寻址模式写满 1024 字节自动回卷到起点。

use defmt::info;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::{Blocking, delay::Delay, gpio::Output, spi::master::Spi};
use ssd1306::{
    Ssd1306,
    command::AddrMode,
    mode::BasicMode,
    prelude::{DisplayRotation, DisplaySize128x64, SPIInterface},
};

/// 屏宽（像素）。
pub const WIDTH: u16 = 128;
/// 屏高（像素）。
pub const HEIGHT: u16 = 64;
/// GDDRAM 页数：每页 8 行。
const PAGES: u16 = HEIGHT / 8;

/// 清屏用的空帧，长度与整屏 GDDRAM 一致。
const BLANK: [u8; (WIDTH * PAGES) as usize] = [0; (WIDTH * PAGES) as usize];

/// SPI 设备（CS 由 `ExclusiveDevice` 按事务翻转）+ 命令/数据选择线。
type Interface<'d> =
    SPIInterface<ExclusiveDevice<Spi<'d, Blocking>, Output<'d>, Delay>, Output<'d>>;

/// 已初始化的屏。
pub struct Display<'d> {
    /// `ssd1306` 的驱动实例，BasicMode 只做裸写、不自带第二份帧缓冲。
    inner: Ssd1306<Interface<'d>, DisplaySize128x64, BasicMode>,
}

impl<'d> Display<'d> {
    /// 复位、初始化并清屏；地址窗口设为整屏，之后每帧一次 [`Display::flush`] 即可。
    pub fn new(
        spi: Spi<'d, Blocking>,
        dc: Output<'d>,
        mut rst: Output<'d>,
        cs: Output<'d>,
        delay: &mut Delay,
    ) -> Self {
        let device = ExclusiveDevice::new(spi, cs, Delay::new()).expect("cs pin");
        let mut inner = Ssd1306::new(
            SPIInterface::new(device, dc),
            DisplaySize128x64,
            DisplayRotation::Rotate0,
        );
        inner.reset(&mut rst, delay).expect("reset ssd1306");
        // 整块连续写只有 horizontal 模式成立，page 模式写满一页会回卷到同页开头。
        inner
            .init_with_addr_mode(AddrMode::Horizontal)
            .expect("init ssd1306");
        inner
            .set_draw_area((0, 0), (WIDTH as u8, HEIGHT as u8))
            .expect("set draw area");
        inner.draw(&BLANK).expect("clear ssd1306");
        info!("ssd1306 ready {}x{}", WIDTH, HEIGHT);
        Self { inner }
    }

    /// 把整块 1 bit 画布推进 GDDRAM。
    pub fn flush(&mut self, buffer: &[u8]) {
        debug_assert_eq!(buffer.len(), BLANK.len());
        if self.inner.draw(buffer).is_err() {
            info!("ssd1306 write failed");
        }
    }
}
