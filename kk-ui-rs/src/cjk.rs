//! GB2312 常用汉字与中文标点的 16×16 点阵。
//!
//! 数据由 `build.rs` 从仓库自带的 `assets/fonts/unifont.bdf` 生成，落在 `OUT_DIR`，
//! 不开 `cjk-fonts` 特性时整个模块不编译。字形按 Unicode 码点升序排列，查找用二分。
//!
//! 与 [`crate::Font`]（u8g2 位图字体）不同，这里是固定 16×16 的等宽点阵：全角字形宽
//! 16 px，半角字形按 BDF 里的原始宽度（通常 8 px），适合直接画中文而不必生成 u8g2 字库。

use crate::gfx::Frame;

/// 字形边长（像素）。
pub const GLYPH_SIZE: usize = 16;
/// 一个字模的字节数：16 行 × 每行 2 字节，行内 MSB 在左。
pub const GLYPH_LEN: usize = GLYPH_SIZE * 2;
/// 全角字形的步进宽度（像素）。
pub const FULL_WIDTH: i16 = GLYPH_SIZE as i16;

/// 所有字模，每 [`GLYPH_LEN`] 字节一个。
static GLYPHS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/font_cjk.bin"));
/// 与 [`GLYPHS`] 一一对应的像素宽度。
static WIDTHS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/width_cjk.bin"));
/// 与 [`GLYPHS`] 一一对应的码点，u16 小端升序。
static CODEPOINTS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cp_cjk.bin"));

/// 码点在字库里的下标。
fn index(c: char) -> Option<usize> {
    let target = u16::try_from(u32::from(c)).ok()?;
    let mut lo = 0;
    let mut hi = CODEPOINTS.len() / 2;

    while lo < hi {
        let mid = (lo + hi) / 2;
        let at = u16::from_le_bytes([CODEPOINTS[mid * 2], CODEPOINTS[mid * 2 + 1]]);
        match at.cmp(&target) {
            core::cmp::Ordering::Less => lo = mid + 1,
            core::cmp::Ordering::Greater => hi = mid,
            core::cmp::Ordering::Equal => return Some(mid),
        }
    }
    None
}

/// 按字符查 16×16 字模；不在字库里的字符返回 `None`。
pub fn glyph(c: char) -> Option<&'static [u8; GLYPH_LEN]> {
    let at = index(c)?;
    GLYPHS[at * GLYPH_LEN..(at + 1) * GLYPH_LEN].try_into().ok()
}

/// 字形的像素宽度：全角 16、半角（ASCII 等）按 BDF 原始宽度；缺字返回 `None`。
pub fn width(c: char) -> Option<i16> {
    let at = index(c)?;
    WIDTHS.get(at).map(|width| *width as i16)
}

/// 单行文本的像素宽度；缺字按全角计。
pub fn text_width(text: &str) -> u16 {
    let mut total = 0i32;
    for c in text.chars() {
        total += width(c).unwrap_or(FULL_WIDTH) as i32;
    }
    total.clamp(0, i16::MAX as i32) as u16
}

/// 画单个字形，`x`/`y` 是字形左上角；返回步进宽度，缺字返回 `None`。
pub fn draw_char(frame: &mut Frame<'_>, x: i16, y: i16, c: char) -> Option<i16> {
    let bitmap = glyph(c)?;
    let width = width(c)?;
    for row in 0..GLYPH_SIZE {
        let bits = u16::from_be_bytes([bitmap[row * 2], bitmap[row * 2 + 1]]);
        for col in 0..width as usize {
            let on = bits & (0x8000 >> col) != 0;
            frame.plot_source(x + col as i16, y + row as i16, on);
        }
    }
    Some(width)
}

/// 在画布上画一行文本（顶部对齐，`y` 是字形第一行），返回累计步进宽度。
///
/// 缺字按全角宽度留空，与 [`text_width`] 的估算一致。
pub fn draw_text(frame: &mut Frame<'_>, mut x: i16, y: i16, text: &str) -> i16 {
    let start = x;
    for c in text.chars() {
        x += draw_char(frame, x, y, c).unwrap_or(FULL_WIDTH);
    }
    x - start
}
