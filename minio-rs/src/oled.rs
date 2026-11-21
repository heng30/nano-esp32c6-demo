//! SSD1306 OLED 菜单渲染：I2C 128×64、embedded-graphics 画入帧缓冲。
//! ssd1306 crate 绑 embedded-hal 0.2，与 esp-hal 1.2 的 I2C 不兼容，这里手写最小驱动。
//! 四种风格（Classic / Icon List / Grid / Hero+Dock）按 NV 偏好切换；
//! 屏不在（I2C 无 ACK）时 `init` 报错，main 持 `Option<Display>` 自动降级纯日志。

use alloc::format;
use defmt::info;
use embedded_graphics::{
    geometry::OriginDimensions,
    mono_font::{MonoTextStyle, ascii::FONT_5X8},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};
use esp_hal::i2c::master::I2c;
use minio_rs_core::config::MenuStyle;

/// I2C 频率 400 kHz（由调用方建 I2C 时使用）
pub const I2C_FREQ_KHZ: u32 = 400;
/// SSD1306 7 位地址（SA0 接地）
const I2C_ADDR: u8 = 0x3C;
/// 每行像素高（5×8 字体的行高）
const LINE: i32 = 8;
/// 标题栏占的行数
const TITLE_LINES: i32 = 1;
/// 可用行数（64px / 8px - 标题 1 行）
const ROWS: usize = 7;
/// 每次数据块传输长度
const CHUNK: usize = 128;

/// 显示驱动：esp-hal I2C + 1 KiB 帧缓冲
pub struct Display {
    i2c: I2c<'static, esp_hal::Blocking>,
    buf: [u8; 1024],
}

/// 初始化 OLED；I2C 与引脚由调用方建好传入（接线见 README）
pub fn init(i2c: I2c<'static, esp_hal::Blocking>) -> Result<Display, &'static str> {
    let mut d = Display { i2c, buf: [0u8; 1024] };
    // 标准初始化序列；第一组命令同时充当存在性探测（无 ACK 即报错降级）
    d.cmd(&[0xAE])?; // display off
    d.cmd(&[0xD5, 0x80])?; // clock div
    d.cmd(&[0xA8, 0x3F])?; // multiplex 63
    d.cmd(&[0xD3, 0x00])?; // display offset
    d.cmd(&[0x40])?; // start line 0
    d.cmd(&[0x8D, 0x14])?; // charge pump on
    d.cmd(&[0x20, 0x00])?; // 水平寻址模式
    d.cmd(&[0x21, 0x00, 0x7F])?; // 列范围 0..127
    d.cmd(&[0x22, 0x00, 0x07])?; // 页范围 0..7
    d.cmd(&[0xA1])?; // segment remap
    d.cmd(&[0xC8])?; // COM 扫描方向
    d.cmd(&[0x81, 0xCF])?; // contrast
    d.cmd(&[0xD9, 0xF1])?; // precharge
    d.cmd(&[0xDB, 0x40])?; // VCOM detect
    d.cmd(&[0xA4])?; // RAM 内容输出
    d.cmd(&[0xA6])?; // 正常显示
    d.cmd(&[0xAF])?; // display on
    Ok(d)
}

impl Display {
    /// 发一组命令（控制字节 0x00）
    fn cmd(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        let mut out = [0u8; 8];
        if bytes.len() + 1 > out.len() {
            return Err("cmd too long");
        }
        out[0] = 0x00;
        out[1..1 + bytes.len()].copy_from_slice(bytes);
        self.i2c
            .write(I2C_ADDR, &out[..1 + bytes.len()])
            .map_err(|_| "oled not found (no ack)")
    }

    /// 清帧缓冲（需 `flush` 才生效）
    pub fn clear_buffer(&mut self) {
        self.buf.fill(0);
    }

    /// 把帧缓冲推到屏上；水平寻址模式下顺序写整屏即可
    pub fn flush(&mut self) -> Result<(), &'static str> {
        for chunk in self.buf.chunks(CHUNK) {
            let mut out = [0u8; CHUNK + 1];
            out[0] = 0x40; // 控制字节：数据流
            out[1..].copy_from_slice(chunk);
            self.i2c
                .write(I2C_ADDR, &out)
                .map_err(|_| "i2c flush failed")?;
        }
        Ok(())
    }
}

// embedded-graphics 画布支持：像素按页打包进帧缓冲
impl DrawTarget for Display {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(Point { x, y }, color) in pixels.into_iter() {
            if (0..128).contains(&x) && (0..64).contains(&y) {
                let (x, y) = (x as usize, y as usize);
                let idx = x + (y / 8) * 128;
                let mask = 1u8 << (y % 8);
                if color == BinaryColor::On {
                    self.buf[idx] |= mask;
                } else {
                    self.buf[idx] &= !mask;
                }
            }
        }
        Ok(())
    }
}

impl OriginDimensions for Display {
    fn size(&self) -> Size {
        Size::new(128, 64)
    }
}

/// 渲染当前列表到屏；无屏直接返回
pub fn render_list(
    display: &mut Option<Display>,
    style: MenuStyle,
    title: &str,
    names: &[&str],
    cursor: usize,
) {
    let Some(d) = display else {
        return;
    };
    d.clear_buffer();

    // 标题：Hero+Dock 用反白条，其余纯文本
    if matches!(style, MenuStyle::HeroDock) {
        let _ = Rectangle::new(Point::new(0, 0), Size::new(128, LINE as u32))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(d);
        draw_text(d, title, 0, 0, BinaryColor::Off);
    } else {
        draw_text(d, title, 0, 0, BinaryColor::On);
    }

    let top = TITLE_LINES * LINE;
    for (i, name) in names.iter().enumerate() {
        let selected = i == cursor;
        let (x, y) = match style {
            // Grid：两列各 64px，列优先排布
            MenuStyle::Grid => {
                let col = (i / ROWS) as i32;
                let row = (i % ROWS) as i32;
                (col * 64, top + row * LINE)
            }
            _ => (0, top + i as i32 * LINE),
        };
        // Icon List / Grid 选中行整行反白
        let inverted = selected && !matches!(style, MenuStyle::Classic | MenuStyle::HeroDock);
        if inverted {
            let _ = Rectangle::new(Point::new(x, y), Size::new(64, LINE as u32))
                .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                .draw(d);
        }
        let color = if inverted {
            BinaryColor::Off
        } else {
            BinaryColor::On
        };
        let prefix: &str = match style {
            MenuStyle::Classic | MenuStyle::HeroDock => {
                if selected { "> " } else { "  " }
            }
            MenuStyle::IconList => {
                if selected { "> " } else { "\u{25C6} " }
            }
            _ => "",
        };
        draw_text(d, &format!("{}{}", prefix, name), x, y, color);
    }

    let _ = d.flush();
}

/// 屏保：清屏（日志侧内容不受影响）
pub fn blank(display: &mut Option<Display>) {
    let Some(d) = display else {
        return;
    };
    d.clear_buffer();
    let _ = d.flush();
    info!("oled blanked (screensaver)");
}

/// 画一行文字，y 是像素顶（基线取 Top）
fn draw_text(d: &mut Display, text: &str, x: i32, y: i32, color: BinaryColor) {
    let _ = Text::with_baseline(
        text,
        Point::new(x, y),
        MonoTextStyle::new(&FONT_5X8, color),
        Baseline::Top,
    )
    .draw(d);
}
