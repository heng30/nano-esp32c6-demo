//! 画布基础语义：裁剪窗口、绘图模式与圆角矩形轮廓。
//!
//! 这些断言对应 C 版 KK_OLED 的行为，改动绘制层时应先过这里。

use kk_ui::{DrawMode, Frame};

#[test]
fn clip_window_limits_drawing() {
    let mut frame = Frame::new();
    frame.set_clip(10, 10, 4, 4);
    frame.draw_box(0, 0, 128, 64);
    for y in 0..64 {
        for x in 0..128 {
            let inside = (10..14).contains(&x) && (10..14).contains(&y);
            assert_eq!(frame.pixel(x, y), inside, "pixel {x},{y}");
        }
    }
}

#[test]
fn xor_is_its_own_inverse() {
    let mut frame = Frame::new();
    frame.draw_box(20, 20, 10, 10);
    let before = *frame.buffer();
    frame.set_draw_mode(DrawMode::Xor);
    frame.draw_rbox(18, 18, 14, 14, 3);
    frame.draw_rbox(18, 18, 14, 14, 3);
    assert_eq!(*frame.buffer(), before);
}

#[test]
fn clear_mode_erases() {
    let mut frame = Frame::new();
    frame.draw_box(0, 0, 20, 20);
    frame.set_draw_mode(DrawMode::Clear);
    frame.draw_box(5, 5, 5, 5);
    assert!(!frame.pixel(7, 7));
    assert!(frame.pixel(0, 0));
}

#[test]
fn rounded_corners_stay_empty() {
    let mut frame = Frame::new();
    frame.draw_rbox(10, 10, 20, 20, 6);
    assert!(!frame.pixel(10, 10), "corner must be rounded away");
    assert!(frame.pixel(20, 20), "center must be filled");
    assert!(frame.pixel(12, 19), "left edge mid-height must be filled");
}

#[test]
fn hline_and_vline_are_clipped_not_wrapped() {
    let mut frame = Frame::new();
    frame.draw_hline(120, 5, 30);
    frame.draw_vline(5, 60, 30);
    assert!(frame.pixel(127, 5));
    assert!(!frame.pixel(0, 6), "hline must not wrap to next row");
    assert!(frame.pixel(5, 63));
    assert!(!frame.pixel(6, 0), "vline must not wrap to next column");
}
