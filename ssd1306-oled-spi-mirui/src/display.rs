//! SSD1306 128×64 OLED 的 SPI 屏：初始化与整帧递交交给 `ssd1306` crate，
//! 这里只多做一件事——把 mirui 的 RGB565 帧缓冲按亮度压成 1 bit 的 GDDRAM 字节。
//!
//! SSD1306 的显存是「每字节同列连续 8 行、bit0 在最上」的页式布局，跟像素行优先的
//! RGB565 完全不同，所以每帧都要整块重排；horizontal 寻址模式下写满 1024 字节自动回卷。

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
const PAGES: usize = (HEIGHT / 8) as usize;
/// 整屏 GDDRAM 字节数。
pub const FRAME_LEN: usize = (WIDTH as usize) * PAGES;

/// 像素亮的亮度阈值（0–255），灰边会落到两侧，1 bit 屏只能二选一。
const LUMA_THRESHOLD: u32 = 128;

/// 清屏用的空帧。
const BLANK: [u8; FRAME_LEN] = [0; FRAME_LEN];

/// SPI 设备（CS 由 `ExclusiveDevice` 按事务翻转）+ 命令/数据选择线。
type Interface<'d> =
    SPIInterface<ExclusiveDevice<Spi<'d, Blocking>, Output<'d>, Delay>, Output<'d>>;

/// 已初始化的屏。
pub struct Display<'d> {
    /// `ssd1306` 的驱动实例，BasicMode 只做裸写、不自带第二份帧缓冲。
    inner: Ssd1306<Interface<'d>, DisplaySize128x64, BasicMode>,
    /// 1 bit 影子显存，长度与整屏 GDDRAM 一致。
    mono: &'static mut [u8; FRAME_LEN],
}

impl<'d> Display<'d> {
    /// 复位、初始化并清屏；地址窗口设为整屏，之后每帧一次 [`Display::push`] 即可。
    pub fn new(
        spi: Spi<'d, Blocking>,
        dc: Output<'d>,
        mut rst: Output<'d>,
        cs: Output<'d>,
        delay: &mut Delay,
        mono: &'static mut [u8; FRAME_LEN],
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
        Self { inner, mono }
    }

    /// 把一整帧 RGB565 压成 1 bit 并推进 GDDRAM；`rgb565` 只会是整屏缓冲。
    pub fn push(&mut self, rgb565: &[u8]) {
        debug_assert_eq!(rgb565.len(), WIDTH as usize * HEIGHT as usize * 2);
        pack_mono(rgb565, self.mono);
        if self.inner.draw(self.mono).is_err() {
            info!("ssd1306 write failed");
        }
    }
}

/// RGB565 → 1 bit GDDRAM：按 Rec.601 亮度取阈值，结果按页式布局打包。
fn pack_mono(rgb565: &[u8], mono: &mut [u8; FRAME_LEN]) {
    mono.fill(0);
    for y in 0..HEIGHT as usize {
        let page = y / 8;
        let bit = 1u8 << (y % 8);
        let row = y * WIDTH as usize;
        for x in 0..WIDTH as usize {
            let at = (row + x) * 2;
            let px = u16::from_le_bytes([rgb565[at], rgb565[at + 1]]);
            let r = u32::from(px >> 11) * 8;
            let g = u32::from((px >> 5) & 0x3f) * 4;
            let b = u32::from(px & 0x1f) * 8;
            if (r * 77 + g * 151 + b * 28) >> 8 >= LUMA_THRESHOLD {
                mono[page * WIDTH as usize + x] |= bit;
            }
        }
    }
}
