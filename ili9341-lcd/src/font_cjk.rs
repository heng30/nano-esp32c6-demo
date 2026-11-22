//! GB2312 常用汉字点阵字模。
//!
//! 数据由 `build.rs` 从 GNU Unifont 生成，落在 `OUT_DIR`，不进仓库；
//! 字模按 Unicode 码点升序排列，查找用二分。

/// 字形边长（像素）。
pub const GLYPH_SIZE: usize = 16;
/// 一个字模的字节数：16 行 × 每行 2 字节，行内 MSB 在左。
pub const GLYPH_LEN: usize = 32;

/// 6763 个汉字，每个 `GLYPH_LEN` 字节。
static GLYPHS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/font_cjk.bin"));
/// 与 `GLYPHS` 一一对应的码点，u16 小端升序。
static CODEPOINTS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cp_cjk.bin"));

/// 按字符查 16×16 字模；不在 GB2312 汉字区里的字符返回 `None`。
pub fn glyph(c: char) -> Option<&'static [u8; GLYPH_LEN]> {
    let target = u16::try_from(u32::from(c)).ok()?;
    let mut lo = 0;
    let mut hi = CODEPOINTS.len() / 2;

    while lo < hi {
        let mid = (lo + hi) / 2;
        let at = u16::from_le_bytes([CODEPOINTS[mid * 2], CODEPOINTS[mid * 2 + 1]]);
        match at.cmp(&target) {
            core::cmp::Ordering::Less => lo = mid + 1,
            core::cmp::Ordering::Greater => hi = mid,
            core::cmp::Ordering::Equal => {
                let start = mid * GLYPH_LEN;
                return GLYPHS[start..start + GLYPH_LEN].try_into().ok();
            }
        }
    }
    None
}
