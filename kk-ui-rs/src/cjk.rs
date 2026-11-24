//! 内嵌 16×16 点阵字库：ASCII、GB2312 常用汉字与中文标点。
//!
//! 数据由 `build.rs` 从仓库自带的 `assets/fonts/unifont.bdf` 生成，落在 `OUT_DIR`，
//! 不开 `cjk-fonts` 特性时整个模块不编译。字形按 Unicode 码点升序排列，查找与绘制
//! 都在 [`Glyphs`] 里。启用特性时这就是全局默认字体。

use crate::font::Glyphs;

/// 所有字模，每 [`GLYPH_LEN`](crate::font::GLYPH_LEN) 字节一个。
static BITMAPS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/font_cjk.bin"));
/// 与 [`BITMAPS`] 一一对应的像素宽度。
static WIDTHS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/width_cjk.bin"));
/// 与 [`BITMAPS`] 一一对应的码点，u16 小端升序。
static CODEPOINTS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cp_cjk.bin"));

/// 内嵌字库。
pub static GLYPHS: Glyphs = Glyphs {
    bitmaps: BITMAPS,
    widths: WIDTHS,
    codepoints: CODEPOINTS,
};
