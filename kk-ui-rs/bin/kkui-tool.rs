//! kk-ui 资源工具：从项目里采集字符生成字库表，或把图片转成 XBM 位图。
//!
//! 字模直接从编进本程序的 `assets/fonts/unifont.bdf` 里抠，与库的 `cjk-fonts` 特性
//! 无关，装好后在哪都能跑。能取到的码点由 BDF 决定，取不到的字符会被列出来但不入表。
//!
//! 可打印 ASCII（英文字母、英文标点）始终收录；中文字只取扫描来的：目录只认 `.rs` 文件，
//! 且只取字符串与字符字面量里的字，注释（含 `///` 文档注释）不算。显式传进来的非 `.rs`
//! 文件按纯文本取全部字符，方便喂字符清单。
//!
//! 这是包内唯一的可执行程序，`cargo install --path .` 装的就是它：
//!
//! ```text
//! # 采集字符，输出到 assets/font（目录不存在会自动创建）
//! kkui-tool font ../ili9341-lcd-kkui/src --out assets/font
//! # 图片转 32×32 图标
//! kkui-tool icon logo.png --size 32 -o assets/icon.bin
//! ```

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand};

// 抠字模的代码与 build.rs 共用，见 build/bdf.rs。
#[path = "../build/bdf.rs"]
mod bdf;

/// 编进本程序的 GNU Unifont 点阵，与 `cjk-fonts` 用的是同一份字库。
const BDF: &[u8] = include_bytes!("../assets/fonts/unifont.bdf");
/// 字模输出文件名，与 `build.rs` 的产物同构，可直接 `include_bytes!` 进 `Glyphs`。
const FONT_FILE: &str = "font.bin";
/// 宽度表输出文件名，每字一个字节。
const WIDTH_FILE: &str = "width.bin";
/// 码点表输出文件名，每字两个字节小端、升序。
const CODEPOINT_FILE: &str = "cp.bin";
/// 采集到的字符清单，可以再作为 `font` 的输入喂回来。
const TEXT_FILE: &str = "text.txt";
/// 扫描时跳过的目录：编译产物与版本库内部，没有正文。
const SKIP_DIRS: [&str; 3] = ["target", ".git", "node_modules"];
/// 报告缺字时最多列几个，避免刷屏。
const LOST_PREVIEW: usize = 40;

#[derive(Parser)]
#[command(
    about = "kk-ui resources: subset the built-in font, or convert an image to an XBM bitmap"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Collect characters from .rs string and char literals and write the three font tables
    Font {
        /// Project directories or text files to scan
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Output directory, created when missing
        #[arg(short, long, default_value = "font")]
        out: PathBuf,
    },
    /// Convert an image into a 1 bit XBM bitmap (bit order matches Frame::draw_xbm)
    Icon {
        /// Source image: png, jpeg or webp
        image: PathBuf,
        /// Edge length of the output icon in pixels
        #[arg(short, long, default_value_t = 32)]
        size: u32,
        /// Output path, defaults to the input path with a .bin extension
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Font { paths, out } => font(paths, out),
        Command::Icon { image, size, out } => icon(image, *size, out.as_deref()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// 采集字符，按码点升序从内嵌 BDF 里抠出字模，连同字符清单写进 `out`。
fn font(paths: &[PathBuf], out: &Path) -> Result<(), String> {
    // 英文与英文标点始终收录，中文字才靠扫描补。
    let mut chars: BTreeSet<char> = (0x20..=0x7E).filter_map(char::from_u32).collect();
    for path in paths {
        if path.is_dir() {
            scan(path, &mut chars);
        } else if path.is_file() {
            let text = read_text(path)?;
            if is_rust(path) {
                collect_code_chars(&text, &mut chars);
            } else {
                // 显式给的文本文件（例如上一次生成的 text.txt）按原样取全部字符。
                chars.extend(text.chars());
            }
        } else {
            return Err(format!("{}: no such file or directory", path.display()));
        }
    }

    // 控制字符画不出来、超出 u16 的码点进不了码点表，两者都算取不到。
    let mut wanted = Vec::new();
    let mut lost = Vec::new();
    for &c in &chars {
        if c.is_control() {
            continue;
        }
        match u16::try_from(u32::from(c)) {
            Ok(cp) => wanted.push(cp),
            Err(_) => lost.push(c),
        }
    }

    let glyphs = bdf::parse_bdf(BDF, &wanted);
    let mut bitmaps = Vec::new();
    let mut widths = Vec::new();
    let mut codepoints = Vec::new();
    for &cp in &wanted {
        let Some(glyph) = glyphs.get(&cp) else {
            lost.push(char::from_u32(u32::from(cp)).expect("codepoint came from a char"));
            continue;
        };
        bitmaps.extend_from_slice(&glyph.bitmap);
        widths.push(glyph.width);
        codepoints.extend_from_slice(&cp.to_le_bytes());
    }
    lost.sort_unstable();

    let glyph_count = widths.len();
    let text: String = chars.iter().filter(|c| !c.is_control()).collect();
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    write(&out.join(FONT_FILE), &bitmaps)?;
    write(&out.join(WIDTH_FILE), &widths)?;
    write(&out.join(CODEPOINT_FILE), &codepoints)?;
    write(&out.join(TEXT_FILE), text.as_bytes())?;

    println!(
        "{} unique chars, {glyph_count} glyphs, {} KiB of bitmaps",
        chars.len(),
        bitmaps.len() / 1024
    );
    if !lost.is_empty() {
        let preview: Vec<String> = lost.iter().take(LOST_PREVIEW).map(show).collect();
        let tail = if lost.len() > LOST_PREVIEW {
            ", ..."
        } else {
            ""
        };
        println!(
            "{} chars are missing from unifont.bdf: {}{tail}",
            lost.len(),
            preview.join(" ")
        );
    }
    println!(
        "wrote {} ({} bytes)",
        out.join(FONT_FILE).display(),
        bitmaps.len()
    );
    println!(
        "wrote {} ({} bytes)",
        out.join(TEXT_FILE).display(),
        text.len()
    );
    Ok(())
}

/// 递归扫目录，只读 `.rs`：别的文件（图片、字库、markdown）跟界面上的字无关。
fn scan(dir: &Path, chars: &mut BTreeSet<char>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if SKIP_DIRS.iter().any(|skip| entry.file_name() == *skip) {
                continue;
            }
            scan(&path, chars);
        } else if is_rust(&path)
            && let Ok(source) = read_text(&path)
        {
            collect_code_chars(&source, chars);
        }
    }
}

/// 是不是 Rust 源码。
fn is_rust(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "rs")
}

/// 收集一段 Rust 源码里代码位置的字符：只有字符串与字符字面量，注释一律不算。
fn collect_code_chars(source: &str, chars: &mut BTreeSet<char>) {
    // 词法解析失败（残缺片段）就当这文件没有可用的字，不让整趟采集失败。
    if let Ok(tokens) = source.parse::<proc_macro2::TokenStream>() {
        walk_tokens(tokens, chars);
    }
}

/// 递归走词法单元，把字符串/字符字面量里的字收进来。
fn walk_tokens(tokens: proc_macro2::TokenStream, chars: &mut BTreeSet<char>) {
    for token in tokens {
        match token {
            proc_macro2::TokenTree::Group(group) => {
                if !is_doc_attribute(&group) {
                    walk_tokens(group.stream(), chars);
                }
            }
            proc_macro2::TokenTree::Literal(literal) => match syn::Lit::new(literal) {
                syn::Lit::Str(text) => chars.extend(text.value().chars()),
                syn::Lit::Char(c) => {
                    chars.insert(c.value());
                }
                _ => {}
            },
            _ => {}
        }
    }
}

/// `#[doc = "..."]` / `#![doc = "..."]`：`///` 与 `//!` 被重写成的属性，仍属注释，要剔掉。
fn is_doc_attribute(group: &proc_macro2::Group) -> bool {
    if group.delimiter() != proc_macro2::Delimiter::Bracket {
        return false;
    }
    let mut inner = group.stream().into_iter();
    let named_doc =
        matches!(inner.next(), Some(proc_macro2::TokenTree::Ident(name)) if name == "doc");
    let assigned =
        matches!(inner.next(), Some(proc_macro2::TokenTree::Punct(p)) if p.as_char() == '=');
    named_doc && assigned
}

/// 读一个文本文件，失败时把路径带进错误里。
fn read_text(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// 把图片缩到 `size` 见方并二值化，按 XBM 位序（低位在前、每行 8 像素对齐）写出。
///
/// 深色像素算作点亮、透明像素算作熄灭，所以图标应当是透明底加实心图案。
fn icon(image: &Path, size: u32, out: Option<&Path>) -> Result<(), String> {
    if size == 0 || size > 1024 {
        return Err("size must be between 1 and 1024".to_string());
    }
    let source = image::open(image).map_err(|e| format!("{}: {e}", image.display()))?;
    let scaled = image::imageops::resize(
        &source.to_rgba8(),
        size,
        size,
        image::imageops::FilterType::Lanczos3,
    );
    let stride = size.div_ceil(8) as usize;
    let mut bits = vec![0u8; stride * size as usize];
    for (x, y, pixel) in scaled.enumerate_pixels() {
        let luma =
            (299 * u32::from(pixel[0]) + 587 * u32::from(pixel[1]) + 114 * u32::from(pixel[2]))
                / 1000;
        if pixel[3] >= 128 && luma < 128 {
            bits[y as usize * stride + x as usize / 8] |= 1u8 << (x % 8);
        }
    }
    let fallback = image.with_extension("bin");
    let out = out.unwrap_or(&fallback);
    write(out, &bits)?;
    println!(
        "wrote {} ({size}x{size}, {} bytes)",
        out.display(),
        bits.len()
    );
    Ok(())
}

/// 把字符印成好认的形式，控制字符与非 ASCII 走 `U+XXXX`。
fn show(c: &char) -> String {
    if c.is_ascii_graphic() {
        c.to_string()
    } else {
        format!("U+{:04X}", u32::from(*c))
    }
}

/// 写文件，失败时把路径带进错误里。
fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_are_kept_and_comments_are_not() {
        let source = r##"
            //! 文档注释里的字不收
            /// 行文档注释也不收
            /* 块注释
               跨行也不收 */
            const LABEL: &str = "字符串 甲乙";
            const MARK: char = '丙';
            const RAW: &str = r#"原始 丁"#;
            const ESCAPED: char = '\u{620A}';
            // 注释里的 "假字符串" 也不收
        "##;
        let mut chars = BTreeSet::new();
        collect_code_chars(source, &mut chars);
        for c in ['字', '符', '串', '甲', '乙', '丙', '丁', '戊'] {
            assert!(chars.contains(&c), "code char missing: {c}");
        }
        for c in ['注', '释', '块', '文', '档', '假'] {
            assert!(!chars.contains(&c), "comment char leaked: {c}");
        }
    }

    #[test]
    fn directory_scan_takes_rust_files_only() {
        let dir = std::env::temp_dir().join(format!("kkui-tool-scan-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        fs::write(dir.join("a.rs"), "const S: &str = \"甲乙\";\n// 丙\n").expect("write a.rs");
        fs::write(dir.join("b.txt"), "丁戊\n").expect("write b.txt");

        let mut chars = BTreeSet::new();
        scan(&dir, &mut chars);
        fs::remove_dir_all(&dir).expect("failed to remove temp dir");

        assert!(
            chars.contains(&'甲') && chars.contains(&'乙'),
            "literal chars"
        );
        assert!(!chars.contains(&'丙'), "comment char leaked");
        assert!(!chars.contains(&'丁'), "non-rust file should be skipped");
    }
}
