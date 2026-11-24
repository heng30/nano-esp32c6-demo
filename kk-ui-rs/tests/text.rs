//! 共享文本缓冲的 UTF-8 截断语义：超出容量时不许留下半个字符。

use kk_ui::Text;

fn assert_content(text: &Text, expected: &str) {
    text.with(|value| assert_eq!(value, expected));
}

#[test]
fn literal_truncates_on_character_boundary() {
    // 12 个三字节字符共 36 字节，容量 32 字节，应保留前 10 个。
    assert_content(
        &Text::literal("中文中文中文中文中文中文"),
        "中文中文中文中文中文",
    );
    // 未超容量时原样保留。
    assert_content(&Text::literal("128x64@2x"), "128x64@2x");
}

#[test]
fn set_truncates_on_character_boundary() {
    let text = Text::new();
    text.set("确定退出并重启设备请稍候");
    assert_content(&text, "确定退出并重启设备请");
    // 再写入更短的内容不应残留上一次的尾巴。
    text.set("开");
    assert_content(&text, "开");
}
