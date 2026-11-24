//! kk-ui 资源工具：从项目里采集字符生成字库表，或把图片转成 XBM 位图。
//!
//! 字模直接从编进本程序的 `assets/fonts/unifont.bdf` 里抠，与库的 `cjk-fonts` 特性
//! 无关，装好后在哪都能跑。能取到的码点由 BDF 决定，取不到的字符会被列出来但不入表。
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
    /// Collect every character used under the given paths and write the three font tables
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
    let mut chars = BTreeSet::new();
    for path in paths {
        if path.is_dir() {
            scan(path, &mut chars);
        } else if path.is_file() {
            let text = read_text(path)?;
            chars.extend(text.chars());
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

/// 读目录下所有能按 UTF-8 解码的文件；二进制文件（图片、字库）自动跳过。
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
        } else if let Ok(text) = read_text(&path) {
            chars.extend(text.chars());
        }
    }
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
