//! 画布基础语义：裁剪窗口、绘图模式与圆角矩形轮廓。
//!
//! 这些断言对应 C 版 KK_OLED 的行为，改动绘制层时应先过这里。

use kk_ui::{DrawMode, Frame};

/// 128×64 画布的帧缓冲。
fn buffer_128x64() -> [u8; 1024] {
    [0u8; Frame::buffer_len(128, 64)]
}

#[test]
fn clip_window_limits_drawing() {
    let mut buffer = buffer_128x64();
    let mut frame = Frame::new(&mut buffer, 128, 64);
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
    let mut buffer = buffer_128x64();
    let mut frame = Frame::new(&mut buffer, 128, 64);
    frame.draw_box(20, 20, 10, 10);
    let before = frame.buffer().to_vec();
    frame.set_draw_mode(DrawMode::Xor);
    frame.draw_rbox(18, 18, 14, 14, 3);
    frame.draw_rbox(18, 18, 14, 14, 3);
    assert_eq!(frame.buffer(), before.as_slice());
}

#[test]
fn clear_mode_erases() {
    let mut buffer = buffer_128x64();
    let mut frame = Frame::new(&mut buffer, 128, 64);
    frame.draw_box(0, 0, 20, 20);
    frame.set_draw_mode(DrawMode::Clear);
    frame.draw_box(5, 5, 5, 5);
    assert!(!frame.pixel(7, 7));
    assert!(frame.pixel(0, 0));
}

#[test]
fn rounded_corners_stay_empty() {
    let mut buffer = buffer_128x64();
    let mut frame = Frame::new(&mut buffer, 128, 64);
    frame.draw_rbox(10, 10, 20, 20, 6);
    assert!(!frame.pixel(10, 10), "corner must be rounded away");
    assert!(frame.pixel(20, 20), "center must be filled");
    assert!(frame.pixel(12, 19), "left edge mid-height must be filled");
}

#[test]
fn hline_and_vline_are_clipped_not_wrapped() {
    let mut buffer = buffer_128x64();
    let mut frame = Frame::new(&mut buffer, 128, 64);
    frame.draw_hline(120, 5, 30);
    frame.draw_vline(5, 60, 30);
    assert!(frame.pixel(127, 5));
    assert!(!frame.pixel(0, 6), "hline must not wrap to next row");
    assert!(frame.pixel(5, 63));
    assert!(!frame.pixel(6, 0), "vline must not wrap to next column");
}

#[test]
fn canvas_size_comes_from_the_caller() {
    let mut buffer = [0u8; Frame::buffer_len(32, 20)];
    let mut frame = Frame::new(&mut buffer, 32, 20);
    assert_eq!((frame.width(), frame.height()), (32, 20));
    frame.draw_box(0, 0, 100, 100);
    assert_eq!(frame.buffer().len(), 32 * 3, "20 rows need 3 pages");
    assert!(frame.pixel(31, 19));
    assert!(!frame.pixel(31, 20), "rows past the canvas must stay empty");
    assert!(
        !frame.pixel(32, 0),
        "columns past the canvas must stay empty"
    );
}

#[test]
fn expand_row_repeats_pixels_integer_times() {
    let mut buffer = buffer_128x64();
    let mut frame = Frame::new(&mut buffer, 128, 64);
    frame.draw_box(1, 0, 2, 1);

    let mut row = [0u8; 8];
    assert_eq!(frame.expand_row(0, 2, 1u8, 0u8, &mut row), 8);
    assert_eq!(row, [0, 0, 1, 1, 1, 1, 0, 0]);

    let mut short = [0u8; 5];
    assert_eq!(frame.expand_row(0, 2, 1u8, 0u8, &mut short), 4, "only two columns fit");
    assert_eq!(short, [0, 0, 1, 1, 0]);

    assert_eq!(frame.expand_row(0, 0, 1u8, 0u8, &mut row), 0, "zero scale");
    assert_eq!(frame.expand_row(-1, 2, 1u8, 0u8, &mut row), 8, "off-canvas row");
    assert_eq!(row, [0u8; 8], "off-canvas row must be all off");
}
