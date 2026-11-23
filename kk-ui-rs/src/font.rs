//! u8g2 字体数据解析与 1 bit 文字绘制。
//!
//! 只实现 UI 实际用到的参考高度模式（`REF_ALL`）与顶部对齐：字形坐标 `y` 表示
//! 参考高度顶部。数据格式与 u8g2 一致，因此可以直接复用上游生成的字库。

use crate::gfx::Frame;

/// 字体固定头部长度，头部之后是字形记录。
const HEADER_SIZE: usize = 23;

/// 带静态数据的字体。
pub struct Font {
    data: &'static [u8],
}

/// 从 23 字节头部解析出的全局压缩参数与索引偏移。
struct Info {
    bits_per_0: u8,
    bits_per_1: u8,
    bits_width: u8,
    bits_height: u8,
    bits_x: u8,
    bits_y: u8,
    bits_advance: u8,
    max_height: u8,
    y_offset: i8,
    ascent_text: i8,
    descent_text: i8,
    start_upper: u16,
    start_lower: u16,
    start_unicode: u16,
}

/// 字形度量与 RLE 位流位置。
struct Glyph {
    width: u8,
    height: u8,
    x_offset: i8,
    y_offset: i8,
    advance: i8,
    bits: BitReader,
}

/// LSB-first 位流读取位置。
struct BitReader {
    data: &'static [u8],
    index: usize,
    bit: u8,
}

impl BitReader {
    fn byte(&self, index: usize) -> u16 {
        self.data.get(index).copied().unwrap_or(0) as u16
    }

    /// 读取 `count` 位无符号值。
    fn read(&mut self, count: u8) -> u8 {
        if count == 0 {
            return 0;
        }
        let mut value = self.byte(self.index) >> self.bit;
        let total = self.bit + count;
        if total > 8 {
            value |= self.byte(self.index + 1) << (8 - self.bit);
        }
        self.index += (total >> 3) as usize;
        self.bit = total & 7;
        (value & ((1u16 << count) - 1)) as u8
    }

    /// 读取 u8g2 偏置编码的有符号字段。
    fn read_signed(&mut self, count: u8) -> i8 {
        if count == 0 {
            return 0;
        }
        (self.read(count) as i16 - (1i16 << (count - 1))) as i8
    }
}

fn be16(data: &[u8], index: usize) -> u16 {
    let hi = data.get(index).copied().unwrap_or(0);
    let lo = data.get(index + 1).copied().unwrap_or(0);
    (hi as u16) << 8 | lo as u16
}

impl Font {
    /// 包装一份 u8g2 字体数据。
    pub const fn new(data: &'static [u8]) -> Self {
        Self { data }
    }

    fn info(&self) -> Info {
        let data = self.data;
        Info {
            bits_per_0: data[2],
            bits_per_1: data[3],
            bits_width: data[4],
            bits_height: data[5],
            bits_x: data[6],
            bits_y: data[7],
            bits_advance: data[8],
            max_height: data[10],
            y_offset: data[12] as i8,
            ascent_text: data[13] as i8,
            descent_text: data[14] as i8,
            start_upper: be16(data, 17),
            start_lower: be16(data, 19),
            start_unicode: be16(data, 21),
        }
    }

    /// 当前参考高度模式下的上升量。
    pub fn ascent(&self) -> i16 {
        let info = self.info();
        let all_ascent = info.max_height as i16 + info.y_offset as i16;
        (info.ascent_text as i16).max(all_ascent)
    }

    /// 当前参考高度模式下的下降量（负值）。
    pub fn descent(&self) -> i16 {
        let info = self.info();
        (info.descent_text as i16).min(info.y_offset as i16)
    }

    /// 查找字形的压缩数据。
    fn glyph_data(&self, codepoint: u16) -> Option<usize> {
        let data = self.data;
        let info = self.info();
        let mut cursor = HEADER_SIZE;
        if codepoint <= 0x00FF {
            if codepoint >= b'a' as u16 {
                cursor += info.start_lower as usize;
            } else if codepoint >= b'A' as u16 {
                cursor += info.start_upper as usize;
            }
            while data.get(cursor + 1).copied()? != 0 {
                if data[cursor] == codepoint as u8 {
                    return Some(cursor + 2);
                }
                cursor += data[cursor + 1] as usize;
            }
            return None;
        }

        cursor += info.start_unicode as usize;
        let mut lookup = cursor;
        loop {
            cursor += be16(data, lookup) as usize;
            let indexed = be16(data, lookup + 2);
            lookup += 4;
            if indexed >= codepoint {
                break;
            }
        }
        loop {
            let encoded = be16(data, cursor);
            if encoded == 0 {
                return None;
            }
            if encoded == codepoint {
                return Some(cursor + 3);
            }
            cursor += data.get(cursor + 2).copied()? as usize;
        }
    }

    /// 从字形数据开头解出度量字段。
    fn decode(&self, index: usize) -> Option<Glyph> {
        let info = self.info();
        let mut bits = BitReader {
            data: self.data.get(index..)?,
            index: 0,
            bit: 0,
        };
        let glyph = Glyph {
            width: bits.read(info.bits_width),
            height: bits.read(info.bits_height),
            x_offset: bits.read_signed(info.bits_x),
            y_offset: bits.read_signed(info.bits_y),
            advance: bits.read_signed(info.bits_advance),
            bits,
        };
        Some(glyph)
    }

    /// 查找请求字形；缺字时回退为空格的度量，并标记为不绘制。
    fn glyph_or_space(&self, codepoint: u16) -> Option<(Glyph, bool)> {
        if let Some(index) = self.glyph_data(codepoint) {
            return self.decode(index).map(|glyph| (glyph, true));
        }
        if codepoint == 0x0020 {
            return None;
        }
        self.glyph_data(0x0020)
            .and_then(|index| self.decode(index))
            .map(|glyph| (glyph, false))
    }

    /// 把一段游程展开到画布。
    #[allow(clippy::too_many_arguments)]
    fn draw_run(
        &self,
        frame: &mut Frame,
        x: i16,
        y: i16,
        glyph: &Glyph,
        position: &mut u32,
        length: u8,
        on: bool,
    ) {
        let total = glyph.width as u32 * glyph.height as u32;
        let vref = self.ascent() + 1;
        let mut remaining = length as u32;
        while remaining > 0 && *position < total {
            let local_x = (*position % glyph.width as u32) as i16;
            let local_y = (*position / glyph.width as u32) as i16;
            let target_x = x + glyph.x_offset as i16 + local_x;
            let target_y = y + vref - glyph.height as i16 - glyph.y_offset as i16 + local_y;
            frame.plot_source(target_x, target_y, on);
            *position += 1;
            remaining -= 1;
        }
    }

    /// 解压一个字形的 0/1 游程并绘制。
    fn render(&self, frame: &mut Frame, x: i16, y: i16, glyph: &Glyph) {
        let info = self.info();
        let mut bits = BitReader {
            data: glyph.bits.data,
            index: glyph.bits.index,
            bit: glyph.bits.bit,
        };
        if glyph.width == 0 || glyph.height == 0 {
            return;
        }
        let total = glyph.width as u32 * glyph.height as u32;
        let mut position = 0;
        while position < total {
            let zeros = bits.read(info.bits_per_0);
            let ones = bits.read(info.bits_per_1);
            loop {
                self.draw_run(frame, x, y, glyph, &mut position, zeros, false);
                self.draw_run(frame, x, y, glyph, &mut position, ones, true);
                if bits.read(1) == 0 || position >= total {
                    break;
                }
            }
        }
    }

    /// 绘制单行 UTF-8 字符串，返回累计 advance。
    pub fn draw_text(&self, frame: &mut Frame, mut x: i16, y: i16, text: &str) -> i16 {
        let mut total = 0i32;
        for character in text.chars() {
            let codepoint = u16::try_from(character as u32).unwrap_or(0x0020);
            let Some((glyph, found)) = self.glyph_or_space(codepoint) else {
                continue;
            };
            if found {
                self.render(frame, x, y, &glyph);
            }
            total += glyph.advance as i32;
            x = (x as i32 + glyph.advance as i32) as i16;
        }
        total.clamp(i16::MIN as i32, i16::MAX as i32) as i16
    }

    /// 计算字符串可见像素宽度：尾字形按真实右边界计，不留 advance 空白。
    pub fn text_width(&self, text: &str) -> u16 {
        let mut width = 0i32;
        let mut last_advance = 0i16;
        let mut last_visible = 0i16;
        let mut has_character = false;
        for character in text.chars() {
            let codepoint = u16::try_from(character as u32).unwrap_or(0x0020);
            match self.glyph_or_space(codepoint) {
                Some((glyph, _)) => {
                    last_advance = glyph.advance as i16;
                    last_visible = glyph.width as i16 + glyph.x_offset as i16;
                    width += glyph.advance as i32;
                }
                None => {
                    last_advance = 0;
                    last_visible = 0;
                }
            }
            has_character = true;
        }
        if has_character {
            width -= last_advance as i32;
            width += last_visible as i32;
        }
        width.clamp(0, u16::MAX as i32) as u16
    }
}
