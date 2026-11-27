//! SSD1306 128×64 OLED 的 SPI 驱动：只写不读，整帧按页一次递交。
//!
//! kk-ui 的帧缓冲与 SSD1306 的 GDDRAM 布局一致（每字节是同列连续 8 行、bit0 在最上），
//! 所以一帧就是一次连续写，不做位序转换、也不逐行拼包。

use defmt::info;
use esp_hal::{Blocking, delay::Delay, gpio::Output, spi::master::Spi};

/// 屏宽（像素）。
pub const WIDTH: u16 = 128;
/// 屏高（像素）。
pub const HEIGHT: u16 = 64;
/// GDDRAM 页数：每页 8 行。
const PAGES: u8 = (HEIGHT / 8) as u8;

/// 初始化命令序列：`(命令, 参数)`，参数同样走命令通道。
const INIT: &[(u8, &[u8])] = &[
    (0xAE, &[]),     // display off
    (0xD5, &[0x80]), // clock divide ratio
    (0xA8, &[0x3F]), // multiplex ratio 1/64
    (0xD3, &[0x00]), // display offset 0
    (0x40, &[]),     // start line 0
    (0x8D, &[0x14]), // charge pump on
    (0x20, &[0x00]), // horizontal addressing mode
    (0xA1, &[]),     // segment remap
    (0xC8, &[]),     // COM scan direction reversed
    (0xDA, &[0x12]), // COM pins configuration
    (0x81, &[0xCF]), // contrast
    (0xD9, &[0xF1]), // pre-charge period
    (0xDB, &[0x40]), // VCOMH deselect level
    (0xA4, &[]),     // display follows RAM
    (0xA6, &[]),     // normal (not inverted)
    (0x2E, &[]),     // deactivate scroll
    (0xAF, &[]),     // display on
];

/// 已初始化的屏：SPI 总线与 DC/CS 两根控制线。
pub struct Display<'d> {
    /// SPI 主机，8 位一拍，无 MISO。
    spi: Spi<'d, Blocking>,
    /// 数据/命令选择：低=命令，高=数据。
    dc: Output<'d>,
    /// 片选，低有效；整帧传输期间保持低。
    cs: Output<'d>,
}

impl<'d> Display<'d> {
    /// 复位、初始化并清屏。
    pub fn new(
        spi: Spi<'d, Blocking>,
        dc: Output<'d>,
        mut rst: Output<'d>,
        cs: Output<'d>,
        delay: &mut Delay,
    ) -> Self {
        rst.set_low();
        delay.delay_millis(20);
        rst.set_high();
        delay.delay_millis(20);

        let mut this = Self { spi, dc, cs };
        this.cs.set_low();
        for (command, args) in INIT {
            this.commands(&[*command]);
            this.commands(args);
        }
        // 上电后 GDDRAM 是随机值，先整屏清一遍再放开片选。
        this.commands(&[0x21, 0, (WIDTH - 1) as u8]);
        this.commands(&[0x22, 0, PAGES - 1]);
        this.data(&[0u8; WIDTH as usize * PAGES as usize]);
        this.cs.set_high();
        info!("ssd1306 ready {}x{}", WIDTH, HEIGHT);
        this
    }

    /// 把整块 1 bit 画布推进 GDDRAM。
    pub fn flush(&mut self, buffer: &[u8]) {
        debug_assert_eq!(buffer.len(), WIDTH as usize * PAGES as usize);
        self.cs.set_low();
        // 每帧重设窗口：horizontal 模式下写满会自动回卷，重设只为不依赖这个行为。
        self.commands(&[0x21, 0, (WIDTH - 1) as u8]);
        self.commands(&[0x22, 0, PAGES - 1]);
        self.data(buffer);
        self.cs.set_high();
    }

    /// 命令通道：DC 拉低后写若干字节。
    fn commands(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.dc.set_low();
        self.write(bytes);
    }

    /// 数据通道：DC 拉高后写若干字节。
    fn data(&mut self, bytes: &[u8]) {
        self.dc.set_high();
        self.write(bytes);
    }

    /// SPI 写，失败只记日志：屏掉线不该让固件停摆。
    fn write(&mut self, bytes: &[u8]) {
        if let Err(error) = self.spi.write(bytes) {
            info!("spi write failed: {:?}", error);
        }
    }
}
