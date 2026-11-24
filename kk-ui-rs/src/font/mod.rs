//! 全局字体缓存：整个 UI 只有一个当前字体，[`Frame`](crate::gfx::Frame) 的文字
//! 接口直接读它，页面与弹窗不再各自持有字体。
//!
//! 两种字体来源：外部 u8g2 位图字体（[`Font`]）与 `cjk-fonts` 特性内嵌的 16×16
//! 点阵。启用 `cjk-fonts` 时默认就是内嵌点阵，程序初始化无需任何调用；要换外部
//! 字体用 [`set_font`]，换回内嵌点阵用 [`set_builtin_cjk`]。不开 `cjk-fonts` 时
//! 初始未设置字体，文字绘制为空操作，须先 [`set_font`]。

use core::sync::atomic::{AtomicPtr, Ordering};

#[cfg(feature = "cjk-fonts")]
use core::sync::atomic::AtomicBool;
#[cfg(feature = "cjk-fonts")]
use crate::cjk;
use crate::gfx::Frame;

mod u8g2;

pub use u8g2::Font;

/// 当前是否用内嵌 CJK 点阵。
#[cfg(feature = "cjk-fonts")]
static BUILTIN: AtomicBool = AtomicBool::new(true);

/// 外部 u8g2 字体槽；空指针表示未设置。
static U8G2: AtomicPtr<Font> = AtomicPtr::new(core::ptr::null_mut());

/// 全局改用外部 u8g2 字体。
pub fn set_font(font: &'static Font) {
    #[cfg(feature = "cjk-fonts")]
    BUILTIN.store(false, Ordering::Relaxed);
    U8G2.store(font as *const Font as *mut Font, Ordering::Relaxed);
}

/// 全局改回内嵌 CJK 点阵。
#[cfg(feature = "cjk-fonts")]
pub fn set_builtin_cjk() {
    BUILTIN.store(true, Ordering::Relaxed);
}

/// 当前字体来源。
enum Current {
    /// 未设置，文字绘制为空操作。
    None,
    /// 外部 u8g2 字体。
    U8g2(&'static Font),
    /// 内嵌 CJK 点阵。
    #[cfg(feature = "cjk-fonts")]
    BuiltinCjk,
}

/// 读当前字体；指针只可能来自 [`set_font`] 的 `&'static`，存在期间永远有效。
fn current() -> Current {
    #[cfg(feature = "cjk-fonts")]
    if BUILTIN.load(Ordering::Relaxed) {
        return Current::BuiltinCjk;
    }
    let font = U8G2.load(Ordering::Relaxed);
    // SAFETY: 指针来自 set_font 存入的 &'static Font，生命周期与程序一致。
    match if font.is_null() {
        None
    } else {
        Some(unsafe { &*font })
    } {
        Some(font) => Current::U8g2(font),
        None => Current::None,
    }
}

/// 按当前字体绘制单行文本，返回累计步进宽度。
pub(crate) fn draw_text(frame: &mut Frame<'_>, x: i16, y: i16, text: &str) -> i16 {
    match current() {
        Current::U8g2(font) => font.draw_text(frame, x, y, text),
        #[cfg(feature = "cjk-fonts")]
        Current::BuiltinCjk => cjk::draw_text(frame, x, y, text),
        Current::None => 0,
    }
}

/// 按当前字体计算单行文本的像素宽度。
pub(crate) fn text_width(text: &str) -> u16 {
    match current() {
        Current::U8g2(font) => font.text_width(text),
        #[cfg(feature = "cjk-fonts")]
        Current::BuiltinCjk => cjk::text_width(text),
        Current::None => 0,
    }
}
