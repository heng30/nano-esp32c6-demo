//! 构建时从仓库自带的 GNU Unifont BDF 里抠出 GB2312 汉字点阵，写进 `OUT_DIR`，
//! 由 `src/font_cjk.rs` 用 `include_bytes!` 嵌入固件。
//!
//! 字库是仓库公共资源 `../assets/unifont.bdf`，不再依赖本机安装；缺文件时直接失败，
//! 静默生成空表只会让屏幕一片空白。

use std::{
    collections::BTreeMap,
    env, fs,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

/// 字形边长（像素）。
const GLYPH_SIZE: usize = 16;
/// 一个字模的字节数：16 行 × 每行 2 字节。
const GLYPH_LEN: usize = GLYPH_SIZE * 2;

/// 仓库公共资源里的 BDF，构建不依赖本机装的字体。
fn bdf_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/unifont.bdf")
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../assets/unifont.bdf");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is not set"));
    let bdf = bdf_path();
    assert!(
        bdf.is_file(),
        "{} is missing; the CJK font data is tracked in the repo, restore it with git",
        bdf.display()
    );

    let wanted = gb2312_codepoints();
    let glyphs = parse_bdf(&bdf, &wanted);
    if glyphs.len() != wanted.len() {
        println!(
            "cargo:warning={} of {} GB2312 glyphs missing from {}",
            wanted.len() - glyphs.len(),
            wanted.len(),
            bdf.display()
        );
    }

    let mut font = BufWriter::new(
        fs::File::create(out_dir.join("font_cjk.bin")).expect("failed to create font_cjk.bin"),
    );
    let mut codepoints = BufWriter::new(
        fs::File::create(out_dir.join("cp_cjk.bin")).expect("failed to create cp_cjk.bin"),
    );
    for cp in &wanted {
        if let Some(glyph) = glyphs.get(cp) {
            font.write_all(glyph).expect("failed to write glyph");
            codepoints
                .write_all(&cp.to_le_bytes())
                .expect("failed to write codepoint");
        }
    }
    font.flush().expect("failed to flush font_cjk.bin");
    codepoints.flush().expect("failed to flush cp_cjk.bin");

    println!(
        "cargo:warning=cjk font: {} glyphs, {} B + {} B codepoints",
        glyphs.len(),
        glyphs.len() * GLYPH_LEN,
        glyphs.len() * 2
    );
}

/// GB2312 汉字区（0xB0–0xF7 区、0xA1–0xFE 位）里的汉字码点，升序去重。
///
/// 该区在 GBK 与 GB2312 下编码一致，所以用 `encoding_rs::GBK` 解码即可，不必自带码表。
fn gb2312_codepoints() -> Vec<u16> {
    let mut codepoints = Vec::with_capacity(6763);
    for hi in 0xB0..=0xF7u8 {
        for lo in 0xA1..=0xFEu8 {
            let bytes = [hi, lo];
            let (text, _, had_errors) = encoding_rs::GBK.decode(&bytes);
            if had_errors {
                continue;
            }
            let Some(ch) = text.chars().next() else {
                continue;
            };
            if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
                codepoints.push(ch as u16);
            }
        }
    }
    codepoints.sort_unstable();
    codepoints.dedup();
    codepoints
}

/// 从 BDF 里取出 `wanted` 中的字形，返回 码点 → 点阵。
fn parse_bdf(path: &Path, wanted: &[u16]) -> BTreeMap<u16, [u8; GLYPH_LEN]> {
    let file = fs::File::open(path).expect("failed to open bdf");
    let mut glyphs = BTreeMap::new();

    let mut encoding = None;
    let mut width = GLYPH_SIZE;
    let mut rows: Vec<u16> = Vec::with_capacity(GLYPH_SIZE);
    let mut in_bitmap = false;

    for line in BufReader::new(file).lines() {
        let line = line.expect("failed to read bdf");
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("ENCODING") {
            encoding = rest.trim().parse().ok();
        } else if let Some(rest) = line.strip_prefix("BBX") {
            width = rest
                .trim()
                .split_whitespace()
                .next()
                .and_then(|w| w.parse().ok())
                .unwrap_or(GLYPH_SIZE);
        } else if line.starts_with("BITMAP") {
            rows.clear();
            in_bitmap = true;
        } else if line == "ENDCHAR" {
            if let Some(cp) = encoding {
                if wanted.binary_search(&cp).is_ok() {
                    glyphs.insert(cp, to_glyph(&rows, width));
                }
            }
            encoding = None;
            in_bitmap = false;
        } else if in_bitmap {
            if let Ok(value) = u16::from_str_radix(line, 16) {
                rows.push(value);
            }
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
