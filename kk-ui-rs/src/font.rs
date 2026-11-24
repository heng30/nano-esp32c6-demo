//! 16×16 定长点阵字库与全局字体缓存。
//!
//! 字库是「点阵 / 宽度 / 码点」三张平行静态表，行内 MSB 在左、码点升序二分查找，
//! 全角字形步进 16 px、半角按表里的原始宽度。`cjk-fonts` 特性自带一份从 unifont
//! 抠出的字库（[`crate::cjk::GLYPHS`]），也可以用自己的表替换。
//!
//! 整个 UI 只有一个当前字体，[`Frame`](crate::gfx::Frame) 的文字接口直接读它，页面
//! 与弹窗不再各自持有字体。启用 `cjk-fonts` 时默认就是内嵌字库，初始化无需任何调用；
//! 不开特性时初始无字体，文字绘制为空操作，须先 [`set_font`]。

use crate::gfx::Frame;
use core::sync::atomic::{AtomicPtr, Ordering};

/// 字形边长（像素）。
pub const GLYPH_SIZE: usize = 16;
/// 一个字模的字节数：16 行 × 每行 2 字节，行内 MSB 在左。
pub const GLYPH_LEN: usize = GLYPH_SIZE * 2;
/// 全角字形的步进宽度（像素），缺字也按它留空。
pub const FULL_WIDTH: i16 = GLYPH_SIZE as i16;

/// 当前字体槽；空指针表示用内嵌字库。
static CURRENT: AtomicPtr<Glyphs> = AtomicPtr::new(core::ptr::null_mut());

/// 16×16 定长点阵字库。
pub struct Glyphs {
    /// 点阵，每 [`GLYPH_LEN`] 字节一个字模。
    pub bitmaps: &'static [u8],
    /// 与 [`Glyphs::bitmaps`] 一一对应的像素宽度。
    pub widths: &'static [u8],
    /// 与 [`Glyphs::bitmaps`] 一一对应的码点，u16 小端升序。
    pub codepoints: &'static [u8],
}

impl Glyphs {
    /// 码点在字库里的下标。
    fn index(&self, c: char) -> Option<usize> {
        let target = u16::try_from(u32::from(c)).ok()?;
        let mut lo = 0;
        let mut hi = self.codepoints.len() / 2;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let at = u16::from_le_bytes([self.codepoints[mid * 2], self.codepoints[mid * 2 + 1]]);
            match at.cmp(&target) {
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
                core::cmp::Ordering::Equal => return Some(mid),
            }
        }
        None
    }

    /// 按字符查字模；不在字库里的字符返回 `None`。
    pub fn glyph(&self, c: char) -> Option<&'static [u8; GLYPH_LEN]> {
        let at = self.index(c)?;
        self.bitmaps
            .get(at * GLYPH_LEN..(at + 1) * GLYPH_LEN)?
            .try_into()
            .ok()
    }

    /// 字形的像素宽度：全角 16、半角按表里的原始宽度；缺字返回 `None`。
    pub fn width(&self, c: char) -> Option<i16> {
        let at = self.index(c)?;
        self.widths.get(at).map(|width| *width as i16)
    }

    /// 单行文本的像素宽度；缺字按全角计。
    pub fn text_width(&self, text: &str) -> u16 {
        let mut total = 0i32;
        for c in text.chars() {
            total += self.width(c).unwrap_or(FULL_WIDTH) as i32;
        }
        total.clamp(0, i16::MAX as i32) as u16
    }

    /// 画单个字形，`x`/`y` 是字形左上角；返回步进宽度，缺字返回 `None`。
    pub fn draw_char(&self, frame: &mut Frame<'_>, x: i16, y: i16, c: char) -> Option<i16> {
        let bitmap = self.glyph(c)?;
        let width = self.width(c)?;
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
    /// 缺字按全角宽度留空，与 [`Glyphs::text_width`] 的估算一致。
    pub fn draw_text(&self, frame: &mut Frame<'_>, mut x: i16, y: i16, text: &str) -> i16 {
        let start = x;
        for c in text.chars() {
            x += self.draw_char(frame, x, y, c).unwrap_or(FULL_WIDTH);
        }
        x - start
    }
}

/// 设置全局字体，通常在程序初始化时调一次。
pub fn set_font(glyphs: &'static Glyphs) {
    CURRENT.store(glyphs as *const Glyphs as *mut Glyphs, Ordering::Relaxed);
}

/// 读当前字体；指针只可能来自 [`set_font`] 的 `&'static`，存在期间永远有效。
fn current() -> Option<&'static Glyphs> {
    let glyphs = CURRENT.load(Ordering::Relaxed);
    if !glyphs.is_null() {
        // SAFETY: 指针来自 set_font 存入的 &'static Glyphs，生命周期与程序一致。
        return Some(unsafe { &*glyphs });
    }
    #[cfg(feature = "cjk-fonts")]
    return Some(&crate::cjk::GLYPHS);
    #[cfg(not(feature = "cjk-fonts"))]
    None
}

/// 按当前字体绘制单行文本，返回累计步进宽度。
pub(crate) fn draw_text(frame: &mut Frame<'_>, x: i16, y: i16, text: &str) -> i16 {
    match current() {
        Some(glyphs) => glyphs.draw_text(frame, x, y, text),
        None => 0,
    }
}

/// 按当前字体计算单行文本的像素宽度。
pub(crate) fn text_width(text: &str) -> u16 {
    match current() {
        Some(glyphs) => glyphs.text_width(text),
        None => 0,
    }
}
