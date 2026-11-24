//! 内嵌中文点阵：常用汉字、中文标点与全角字符都查得到、画得出。

#![cfg(feature = "cjk-fonts")]

use kk_ui::Frame;
use kk_ui::cjk;

#[test]
fn common_han_and_punctuation_are_embedded() {
    let sample = [
        '中', '文', '汉', '字', '常', '用',
        '，', '。', '、', '：', '；', '！', '？', '“', '”', '‘', '’', '（', '）', '《', '》',
        '—', '…', '·', '　', '％', '０',
    ];
    for c in sample {
        assert!(cjk::glyph(c).is_some(), "missing glyph: {c}");
    }
    assert_eq!(cjk::width('中'), Some(16), "han glyphs are full width");
}

#[test]
fn ascii_letters_and_symbols_are_embedded() {
    // ASCII 大小写与常用符号（含空格）都在字库里，中英混排可以直接画。
    for c in ['A', 'z', '0', '%', ':', ' ', '!', '~'] {
        assert!(cjk::glyph(c).is_some(), "missing ascii glyph: {c}");
    }
    assert!(cjk::width('A').is_some() && cjk::width('A').unwrap() <= 8, "ascii is half width");
}

#[test]
fn missing_characters_return_none() {
    assert!(cjk::glyph('\u{1F600}').is_none(), "emoji is out of range");
    assert!(cjk::glyph('\u{4DBF}').is_none(), "CJK ext-B is out of GB2312 range");
}

#[test]
fn glyphs_are_not_blank_and_draw_onto_the_canvas() {
    let bitmap = cjk::glyph('中').expect("han glyph");
    assert!(bitmap.iter().any(|byte| *byte != 0), "glyph must have pixels");

    let mut buffer = [0u8; Frame::buffer_len(64, 32)];
    let mut frame = Frame::new(&mut buffer, 64, 32);
    assert_eq!(cjk::draw_text(&mut frame, 0, 0, "中文，标点"), 80);
    let lit = (0..32)
        .flat_map(|y| (0..64).map(move |x| (x, y)))
        .filter(|(x, y)| frame.pixel(*x, *y))
        .count();
    assert!(lit > 32, "only {lit} pixels drawn");
}

#[test]
fn text_width_matches_draw_advance() {
    let text = "屏幕亮度：80%";
    let mut buffer = [0u8; Frame::buffer_len(128, 32)];
    let mut frame = Frame::new(&mut buffer, 128, 32);
    let advance = cjk::draw_text(&mut frame, 0, 0, text);
    assert_eq!(cjk::text_width(text) as i16, advance);
}
