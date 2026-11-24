//! 构建期从仓库自带的 GNU Unifont BDF 里抠出 ASCII 与 GB2312 汉字、中文标点，写进
//! `OUT_DIR`，由 `src/cjk.rs` 用 `include_bytes!` 嵌进库。
//!
//! 只有启用 `cjk-fonts` 特性时才真正生成；没开特性时直接返回，库本体不带这些数据。
//! BDF 是仓库自带资源 `assets/fonts/unifont.bdf`，不依赖本机装的字体。

use std::{
    env, fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

// 抠字模的代码与 bin/kkui-tool.rs 共用，见 build/bdf.rs。
include!("build/bdf.rs");

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build/bdf.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_CJK_FONTS");
    if env::var_os("CARGO_FEATURE_CJK_FONTS").is_none() {
        return;
    }
    println!("cargo:rerun-if-changed=assets/fonts/unifont.bdf");

    let bdf = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/unifont.bdf");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is not set"));
    assert!(
        bdf.is_file(),
        "{} is missing; the CJK font data is tracked in the repo, restore it with git",
        bdf.display()
    );

    let data = fs::read(&bdf).unwrap_or_else(|e| panic!("failed to read {}: {e}", bdf.display()));
    let wanted = wanted_codepoints();
    let glyphs = parse_bdf(&data, &wanted);
    if glyphs.len() != wanted.len() {
        println!(
            "cargo:warning={} of {} glyphs missing from {}",
            wanted.len() - glyphs.len(),
            wanted.len(),
            bdf.display()
        );
    }

    let mut font = BufWriter::new(
        fs::File::create(out_dir.join("font_cjk.bin")).expect("failed to create font_cjk.bin"),
    );
    let mut widths = BufWriter::new(
        fs::File::create(out_dir.join("width_cjk.bin")).expect("failed to create width_cjk.bin"),
    );
    let mut codepoints = BufWriter::new(
        fs::File::create(out_dir.join("cp_cjk.bin")).expect("failed to create cp_cjk.bin"),
    );
    for cp in &wanted {
        if let Some(glyph) = glyphs.get(cp) {
            font.write_all(&glyph.bitmap)
                .expect("failed to write glyph");
            widths
                .write_all(&[glyph.width])
                .expect("failed to write width");
            codepoints
                .write_all(&cp.to_le_bytes())
                .expect("failed to write codepoint");
        }
    }
    font.flush().expect("failed to flush font_cjk.bin");
    widths.flush().expect("failed to flush width_cjk.bin");
    codepoints.flush().expect("failed to flush cp_cjk.bin");

    println!(
        "cargo:warning=cjk font: {} glyphs, {} KiB",
        glyphs.len(),
        glyphs.len() * (GLYPH_LEN + 3) / 1024
    );
}

/// 需要的码点：ASCII 与 GB2312 符号区、汉字区，升序去重。
///
/// 符号区（0xA1–0xA9）盖住中文标点、全角 ASCII、序号、制表符等；汉字区
/// （0xB0–0xF7）是 6763 个常用字。两区在 GBK 与 GB2312 下编码一致，用
/// `encoding_rs::GBK` 解码即可，不必自带码表。ASCII 直接按单字节区间列出。
fn wanted_codepoints() -> Vec<u16> {
    let mut codepoints: Vec<u16> = (0x0020..=0x007E).collect();
    for hi in 0xA1..=0xF7u8 {
        for lo in 0xA1..=0xFEu8 {
            let bytes = [hi, lo];
            let (text, _, had_errors) = encoding_rs::GBK.decode(&bytes);
            if had_errors {
                continue;
            }
            // 先绑定再取字符：直接链式调用会撞上临时的 decode 结果被借用。
            let Some(ch) = text.chars().next() else {
                continue;
            };
            if keeps(ch as u32) {
                codepoints.push(ch as u16);
            }
        }
    }
    codepoints.sort_unstable();
    codepoints.dedup();
    codepoints
}

/// 只留中文标点、全角字符与汉字，丢掉 GBK 扩展出来的西里尔/假名等生僻区。
fn keeps(cp: u32) -> bool {
    matches!(cp,
        0x00A0..=0x00FF     // 拉丁补充（·、×、÷）
        | 0x2010..=0x206F   // 通用标点（—、…、“”）
        | 0x2100..=0x21FF   // 序号与箭头
        | 0x2460..=0x24FF   // 带圈数字
        | 0x2500..=0x25FF   // 制表符与几何图形
        | 0x3000..=0x303F   // 中文标点（、。「」）
        | 0x4E00..=0x9FFF   // 汉字
        | 0xFF00..=0xFFEF   // 全角 ASCII 与标点
    )
}
