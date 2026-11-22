//! 模拟时钟：表盘、三根指针，外加 3×5 点阵数字时间。

use slint::platform::software_renderer::Rgb565Pixel;

use super::{Canvas, HEIGHT, WIDTH, sin_q10};

/// 表盘半径（像素）。
const FACE_R: i32 = 40;
/// 表心横坐标。
const CX: i32 = WIDTH as i32 / 2;
/// 表心纵坐标。
const CY: i32 = 46;
/// 起始时间：10:09。
const START_SECONDS: i32 = 10 * 3600 + 9 * 60;
/// 指针起始半径，避开表心。
const HUB_R: i32 = 6;

/// 时、分、秒、刻度的颜色（RGB565）。
const HOUR_COLOR: u16 = 0xffff;
const MIN_COLOR: u16 = 0x07ff;
const SEC_COLOR: u16 = 0xf800;
const TICK_COLOR: u16 = 0x4208;
const DIGIT_COLOR: u16 = 0xffe0;

/// 3×5 点阵数字，每行低三位有效。
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// 一直在走的表。
pub struct Clock;

impl Canvas for Clock {
    fn draw(&mut self, px: &mut [Rgb565Pixel], t_ms: u32) {
        px.fill(Rgb565Pixel(0x0000));

        let total = START_SECONDS + (t_ms / 1000) as i32;
        let sec = total % 60;
        let min = (total / 60) % 60;
        let hour = (total / 3600) % 12;

        // 表盘外圈
        ring(px, FACE_R, FACE_R - 1, TICK_COLOR);
        // 12 个刻度
        for k in 0..12 {
            let deg = k * 30;
            hand(px, deg, 0, FACE_R - 5, FACE_R - 2, TICK_COLOR);
        }

        // 时针带分钟的小数偏移，看着更自然
        hand(px, hour * 30 + min / 2, 2, HUB_R, FACE_R * 5 / 10, HOUR_COLOR);
        hand(px, min * 6, 1, HUB_R, FACE_R * 7 / 10, MIN_COLOR);
        hand(px, sec * 6, 0, HUB_R, FACE_R * 8 / 10, SEC_COLOR);

        // 表心
        fill_rect(px, CX - 1, CY - 1, 3, 3, HOUR_COLOR);

        digits(px, hour, min);
    }
}

/// 画一个以表心为圆心的圆环，只描边。
fn ring(px: &mut [Rgb565Pixel], r_out: i32, r_in: i32, color: u16) {
    for y in (CY - r_out)..=(CY + r_out) {
        for x in (CX - r_out)..=(CX + r_out) {
            if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
                continue;
            }
            let dx = x - CX;
            let dy = y - CY;
            let r2 = dx * dx + dy * dy;
            if r2 <= r_out * r_out && r2 >= r_in * r_in {
                px[y as usize * WIDTH + x as usize] = Rgb565Pixel(color);
            }
        }
    }
}

/// 从 `r_from` 到 `r_to` 沿 `deg` 方向画一段线，`thick` 是额外加粗的像素数。
fn hand(px: &mut [Rgb565Pixel], deg: i32, thick: i32, r_from: i32, r_to: i32, color: u16) {
    for r in r_from..=r_to {
        let x = CX + sin_q10(deg) * r / 1024;
        let y = CY - sin_q10(deg + 90) * r / 1024;
        fill_rect(px, x, y, thick + 1, thick + 1, color);
    }
}

/// 画一块实心矩形，自动裁剪到画布内。
fn fill_rect(px: &mut [Rgb565Pixel], x: i32, y: i32, w: i32, h: i32, color: u16) {
    for dy in 0..h {
        for dx in 0..w {
            let (px_x, px_y) = (x + dx, y + dy);
            if px_x >= 0 && px_y >= 0 && px_x < WIDTH as i32 && px_y < HEIGHT as i32 {
                px[px_y as usize * WIDTH + px_x as usize] = Rgb565Pixel(color);
            }
        }
    }
}

/// 在表盘下方画 `HH:MM`，每个点阵像素放大 3 倍。
fn digits(px: &mut [Rgb565Pixel], hour: i32, min: i32) {
    const SCALE: i32 = 3;
    const ADVANCE: i32 = 4 * SCALE;
    /// 冒号在字符表里的占位值，不会拿去索引 `DIGITS`。
    const COLON: i32 = -1;

    // 12 小时制：0 点显示成 12，这样小时位一定是两位数字
    let hour12 = if hour == 0 { 12 } else { hour };
    let cells = [hour12 / 10, hour12 % 10, COLON, min / 10, min % 10];
    let width = ADVANCE * cells.len() as i32 - SCALE;
    let mut x = (WIDTH as i32 - width) / 2;
    let y = HEIGHT as i32 - 5 * SCALE - 3;

    for cell in cells {
        if cell == COLON {
            fill_rect(
                px,
                x + SCALE - 1,
                y + SCALE,
                SCALE - 1,
                SCALE - 1,
                DIGIT_COLOR,
            );
            fill_rect(
                px,
                x + SCALE - 1,
                y + 3 * SCALE,
                SCALE - 1,
                SCALE - 1,
                DIGIT_COLOR,
            );
        } else {
            for (row, bits) in DIGITS[cell as usize].iter().enumerate() {
                for col in 0..3 {
                    if bits & (1 << (2 - col)) == 0 {
                        continue;
                    }
                    fill_rect(
                        px,
                        x + col * SCALE,
                        y + row as i32 * SCALE,
                        SCALE,
                        SCALE,
                        DIGIT_COLOR,
                    );
                }
            }
        }
        x += ADVANCE;
    }
}
