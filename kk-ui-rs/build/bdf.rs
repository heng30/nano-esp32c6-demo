// 从 GNU Unifont BDF 里抠字模的公共代码，`build.rs` 与 `bin/kkui-tool.rs` 共用。
// 两边分别用 `include!` 与 `#[path]` 引入，所以这里不写 `use`、路径一律写全，
// 免得与宿主文件的导入撞名。

/// 字形边长（像素）。
pub const GLYPH_SIZE: usize = 16;
/// 一个字模的字节数：16 行 × 每行 2 字节。
pub const GLYPH_LEN: usize = GLYPH_SIZE * 2;

/// 一个字形：定长点阵与它的原始像素宽度。
pub struct BdfGlyph {
    /// 16×16 点阵，行内 MSB 在左。
    pub bitmap: [u8; GLYPH_LEN],
    /// BDF 里的字形宽度（像素），半角 8、全角 16。
    pub width: u8,
}

/// 从 BDF 数据里取出 `wanted` 中的字形，返回 码点 → 点阵。
pub fn parse_bdf(data: &[u8], wanted: &[u16]) -> std::collections::BTreeMap<u16, BdfGlyph> {
    let text = std::str::from_utf8(data).expect("bdf is not valid utf-8");
    let mut glyphs = std::collections::BTreeMap::new();

    let mut encoding = None;
    let mut width = GLYPH_SIZE;
    let mut rows: Vec<u16> = Vec::with_capacity(GLYPH_SIZE);
    let mut in_bitmap = false;

    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("ENCODING") {
            encoding = rest.trim().parse().ok();
        } else if let Some(rest) = line.strip_prefix("BBX") {
            width = rest
                .split_whitespace()
                .next()
                .and_then(|w| w.parse().ok())
                .unwrap_or(GLYPH_SIZE);
        } else if line.starts_with("BITMAP") {
            rows.clear();
            in_bitmap = true;
        } else if line == "ENDCHAR" {
            if let Some(cp) = encoding
                && wanted.binary_search(&cp).is_ok()
            {
                glyphs.insert(
                    cp,
                    BdfGlyph {
                        bitmap: to_glyph(&rows, width),
                        width: width.min(GLYPH_SIZE) as u8,
                    },
                );
            }
            encoding = None;
            in_bitmap = false;
        } else if in_bitmap && let Ok(value) = u16::from_str_radix(line, 16) {
            rows.push(value);
        }
    }

    glyphs
}

/// 把 BDF 的每行整数拼成 16 位宽的定长点阵，窄字形靠左对齐。
fn to_glyph(rows: &[u16], width: usize) -> [u8; GLYPH_LEN] {
    let mut glyph = [0u8; GLYPH_LEN];
    for (row, value) in rows.iter().take(GLYPH_SIZE).enumerate() {
        let value = if width < GLYPH_SIZE {
            value << (GLYPH_SIZE - width)
        } else {
            *value
        };
        glyph[row * 2..row * 2 + 2].copy_from_slice(&value.to_be_bytes());
    }
    glyph
}
