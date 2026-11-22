#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use defmt::info;
use display_interface::{DataFormat, DisplayError, WriteOnlyDataCommand};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    draw_target::DrawTarget,
    pixelcolor::{Rgb565, raw::RawU16},
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    timer::timg::TimerGroup,
};
use ili9341::{DisplaySize240x320, Ili9341, Orientation};
use panic_rtt_target as _;
use static_cell::StaticCell;

mod font_cjk;

esp_bootloader_esp_idf::esp_app_desc!();

/// 横屏后的可视尺寸。
const DISPLAY_W: u16 = 320;
/// 横屏后的可视尺寸。
const DISPLAY_H: u16 = 240;

/// 数据线 GPIO 编号，下标即位序，不连续。
const DATA_PINS: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 18, 19, 20, 21, 22];
/// RS / D/CX：低=命令，高=数据。
const DC_PIN: u8 = 15;
/// WR：上升沿锁存。
const WR_PIN: u8 = 23;

/// GPIO 外设基址（TRM 里的 GPIO 基址）。
const GPIO_BASE: usize = 0x6009_1000;
/// GPIO_OUT_REG：整体写输出电平。
const GPIO_OUT: usize = GPIO_BASE + 0x04;
/// 全部数据线的掩码。
const DATA_MASK: u32 = data_mask();
/// 数据线 + DC + WR，整体写 out 时需要先清掉这些位。
const CTRL_MASK: u32 = DATA_MASK | (1 << DC_PIN) | (1 << WR_PIN);

/// 低 8 位像素值对应的置位掩码，去掉逐位循环。
static SET_LO: [u32; 256] = mask_table(0);
/// 高 8 位像素值对应的置位掩码。
static SET_HI: [u32; 256] = mask_table(8);

const fn data_mask() -> u32 {
    let mut mask = 0u32;
    let mut i = 0;
    while i < DATA_PINS.len() {
        mask |= 1 << DATA_PINS[i];
        i += 1;
    }
    mask
}

/// 生成 `base..base+8` 这 8 位数据线的置位掩码表。
const fn mask_table(base: usize) -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut value = 0;
    while value < 256 {
        let mut mask = 0u32;
        let mut bit = 0;
        while bit < 8 {
            if value & (1 << bit) != 0 {
                mask |= 1 << DATA_PINS[base + bit];
            }
            bit += 1;
        }
        table[value] = mask;
        value += 1;
    }
    table
}

/// 旋转三角形的外接圆半径（像素）。
const TRI_RADIUS: i32 = 90;
/// 几何效果的外接方形边长：既是重绘区域，也是局部帧缓冲的边长。
const GEO_BOX: u16 = 2 * TRI_RADIUS as u16;
/// 中心固定圆的半径（像素）。
const GEO_DOT: i32 = 25;
/// 几何效果的局部帧缓冲：整块算好再一次送出，没有中间态就不会闪。
/// 在 `main` 里初始化一次，效果循环反复复用它（`StaticCell` 只能初始化一次）。
static GEO_FRAME: StaticCell<[u16; GEO_BOX as usize * GEO_BOX as usize]> = StaticCell::new();

/// 正弦波效果波形区高度（像素），也是局部帧缓冲的高度。
const WAVE_H: u16 = 120;
/// 正弦波振幅（像素）。
const WAVE_AMP: i32 = 50;
/// 每列水平像素对应的角度增量（度）。
const WAVE_STEP: u16 = 5;
/// 每帧相位增量（度）。
const WAVE_PHASE_STEP: u16 = 8;
/// 正弦波帧数，约 300 × 45 ms ≈ 13 s。
const WAVE_FRAMES: u16 = 300;
/// 正弦波效果的局部帧缓冲，在 `main` 里初始化一次、循环复用。
static WAVE_FRAME: StaticCell<[u16; DISPLAY_W as usize * WAVE_H as usize]> = StaticCell::new();

/// 小球数量。
const BALL_COUNT: usize = 8;
/// 小球半径（像素）。
const BALL_R: i32 = 12;
/// 位置/速度的定点比例：16 表示 1 像素。
const BALL_ONE: i32 = 16;
/// 小球效果帧数，约 480 × 16 ms ≈ 8 s。
const BALL_FRAMES: u16 = 480;
/// 小球重绘矩形缓冲边长 = 直径 + 两端各 1 像素余量 + 每帧最大位移 5 像素（Q4 的 80）。
/// `BALLS_INIT` 里所有速度绝对值必须 ≤ 80，否则矩形会超出这个缓冲。
const BALL_BOX: usize = 2 * BALL_R as usize + 3 + 5;
/// 小球重绘用的小矩形缓冲，各球串行复用，在 `main` 里初始化一次。
static BALL_BUF: StaticCell<[u16; BALL_BOX * BALL_BOX]> = StaticCell::new();

/// 8 个小球的初始 (x, y, vx, vy)：位置是中心像素，速度是 Q4 定点。
const BALLS_INIT: [(i32, i32, i32, i32); BALL_COUNT] = [
    (40, 40, 70, 50),
    (280, 60, -55, 75),
    (120, 200, 45, -80),
    (260, 190, -75, -55),
    (80, 120, 80, 60),
    (200, 50, -50, 70),
    (160, 140, 65, -65),
    (300, 120, -70, 45),
];
/// 8 个小球的 RGB565 颜色：红、绿、蓝、黄、青、品红、橙、白。
const BALL_COLORS: [u16; BALL_COUNT] = [
    0xf800, 0x07e0, 0x001f, 0xffe0, 0x07ff, 0xf81f, 0xfd20, 0xffff,
];

/// 等离子效果每帧相位的步进。
const PLASMA_SPEED: u16 = 3;
/// 等离子效果帧数，约 300 × 55 ms ≈ 17 s。
const PLASMA_FRAMES: u16 = 300;

/// 滚动字幕文本，只用 ASCII（8×8 字体只覆盖 ASCII）。
const SCROLL_TEXT: &[u8] = b"ESP32-C6 + ILI9341  8080-16 PARALLEL BUS  NO LCD CONTROLLER  ";
/// 字形放大倍数。
const SCROLL_SCALE: usize = 3;
/// 一个字符占的水平像素。
const GLYPH_ADVANCE: i32 = 8 * SCROLL_SCALE as i32;
/// 字幕带高度。
const BAND_H: u16 = 8 * SCROLL_SCALE as u16;
/// 字幕前景色（RGB565 白）。
const SCROLL_FG: u16 = 0xffff;
/// 字幕背景色（RGB565 黑）。
const SCROLL_BG: u16 = 0x0000;
/// 字幕每帧左移的像素。
const SCROLL_SPEED: i32 = 5;
/// 字幕帧数，跑完一整轮文本。
const SCROLL_FRAMES: u16 = 260;

/// 字幕带的渲染缓冲，同样在 `main` 里初始化一次、循环复用。
static BAND: StaticCell<[u16; DISPLAY_W as usize * BAND_H as usize]> = StaticCell::new();

/// 汉字字形的放大倍数：16 px 字形放到 32 px。
const CJK_SCALE: usize = 2;
/// 汉字之间的间距（像素）。
const CJK_GAP: usize = 2;
/// 中文效果显示的文字，全部落在 GB2312 汉字区。
const CJK_TEXT: &str = "中文点阵显示已就绪";
/// 逐字出现的间隔。
const CJK_STEP: Duration = Duration::from_millis(150);
/// 整句显示完后的停留时间。
const CJK_HOLD: Duration = Duration::from_millis(2000);
/// 文字带高度，等于一个放大后的字形高度。
const CJK_BAND_H: u16 = (font_cjk::GLYPH_SIZE * CJK_SCALE) as u16;
/// 中文效果的文字带缓冲，在 `main` 里初始化一次、循环复用。
static CJK_BAND: StaticCell<[u16; DISPLAY_W as usize * CJK_BAND_H as usize]> = StaticCell::new();

/// 屏的 RST 与板子 EN 共复位，没有独立 GPIO。`ili9341` 初始化时会拉低再拉高一次，
/// 这里用空脚顶替，避免占用一个不存在的引脚。
struct NoReset;

impl embedded_hal::digital::ErrorType for NoReset {
    type Error = core::convert::Infallible;
}

impl embedded_hal::digital::OutputPin for NoReset {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// 8080-16 位并口：16 根数据线 + RS(DC) + WR。CS 接地、RD 接 3V3，只写不读。
struct Bus {
    /// 只负责把引脚配成输出并持有所有权，电平由下面的寄存器直写决定。
    _data: [Output<'static>; 16],
    /// RS/D/CX。
    _dc: Output<'static>,
    /// WR。
    _wr: Output<'static>,
    /// `out` 寄存器的影子值：整体写会连带别的引脚一起覆盖，所以先缓存一份。
    shadow: u32,
}

impl Bus {
    fn new(data: [Output<'static>; 16], dc: Output<'static>, wr: Output<'static>) -> Self {
        // 引脚已在 esp-hal 里配成输出，这里读一次当前 out 值当影子
        let shadow = unsafe { (GPIO_OUT as *const u32).read_volatile() };
        Self {
            _data: data,
            _dc: dc,
            _wr: wr,
            shadow,
        }
    }

    /// 一次并口写：摆好数据与 DC，再打一个 WR 脉冲。
    ///
    /// 数据线、DC、WR 都在 GPIO0-31 这一组，用 `out` 寄存器整体覆盖，一个像素只要两次寄存器写。
    fn write(&mut self, value: u16, is_data: bool) {
        let mut set = SET_LO[(value & 0xff) as usize] | SET_HI[(value >> 8) as usize];
        if is_data {
            set |= 1 << DC_PIN;
        }

        let latched = (self.shadow & !CTRL_MASK) | set;
        unsafe {
            (GPIO_OUT as *mut u32).write_volatile(latched);
            (GPIO_OUT as *mut u32).write_volatile(latched | 1 << WR_PIN);
        }
    }
}

impl WriteOnlyDataCommand for Bus {
    fn send_commands(&mut self, cmd: DataFormat<'_>) -> Result<(), DisplayError> {
        let DataFormat::U8Iter(iter) = cmd else {
            return Err(DisplayError::DataFormatNotImplemented);
        };
        for byte in iter {
            self.write(u16::from(byte), false);
        }
        Ok(())
    }

    fn send_data(&mut self, buf: DataFormat<'_>) -> Result<(), DisplayError> {
        match buf {
            // 16 位并口下命令参数走 D[7:0]
            DataFormat::U8Iter(iter) => {
                for byte in iter {
                    self.write(u16::from(byte), true);
                }
            }
            // 像素一个写周期送完整 16 位 RGB565
            DataFormat::U16BEIter(iter) => {
                for pixel in iter {
                    self.write(pixel, true);
                }
            }
            _ => return Err(DisplayError::DataFormatNotImplemented),
        }
        Ok(())
    }
}

/// 库驱动 + 本仓的并口实现。
type Display = Ili9341<Bus, NoReset>;

/// 角度（度）转 sin 定点值 Q10（1024 = 1.0）。Bhaskara 近似，误差 < 2%，够画几何动画。
fn sin_q10(deg: u16) -> i32 {
    let deg = deg % 360;
    let (x, sign) = if deg < 180 {
        (deg as i32, 1)
    } else {
        (deg as i32 - 180, -1)
    };
    let t = x * (180 - x);
    sign * (4 * t * 1024 / (40500 - t))
}

/// 以方框中心为原点的三角形顶点。
fn tri_vertices(angle: u16) -> [Point; 3] {
    let mut pts = [Point::new(0, 0); 3];
    for (i, p) in pts.iter_mut().enumerate() {
        let deg = angle + i as u16 * 120;
        *p = Point::new(
            sin_q10(deg) * TRI_RADIUS / 1024,
            -sin_q10(deg + 90) * TRI_RADIUS / 1024,
        );
    }
    pts
}

/// 向量 ab 与 ap 的叉积，用来判断点落在三角形某条边的哪一侧。
fn cross(a: Point, b: Point, p: Point) -> i32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

fn inside_triangle(pts: &[Point; 3], p: Point) -> bool {
    let a = cross(pts[0], pts[1], p);
    let b = cross(pts[1], pts[2], p);
    let c = cross(pts[2], pts[0], p);
    (a >= 0 && b >= 0 && c >= 0) || (a <= 0 && b <= 0 && c <= 0)
}

/// 效果一：绕中心旋转的三角形 + 不动的圆。
///
/// 每帧只重画外接方形：整块在内存里算好，再一次性 `draw_raw_slice` 送出。
/// 先擦后画是两次大块写入、中间会黑一下，肉眼就是闪；全屏 clear 要 46 ms 更糟。
async fn effect_geometry(display: &mut Display, buf: &mut [u16]) {
    info!("effect: geometry");
    let half = (GEO_BOX / 2) as i32;
    let left = DISPLAY_W / 2 - GEO_BOX / 2;
    let top = DISPLAY_H / 2 - GEO_BOX / 2;
    let dot2 = GEO_DOT * GEO_DOT;
    let tri_color = RawU16::from(Rgb565::CYAN).into_inner();
    let dot_color = RawU16::from(Rgb565::MAGENTA).into_inner();
    let bg_color = RawU16::from(Rgb565::BLACK).into_inner();

    display.clear(Rgb565::BLACK).expect("clear failed");

    for frame in 0..180u16 {
        let pts = tri_vertices(frame * 2);
        for (row, line) in buf.chunks_mut(GEO_BOX as usize).enumerate() {
            let y = row as i32 - half;
            for (col, px) in line.iter_mut().enumerate() {
                let p = Point::new(col as i32 - half, y);
                // 圆画在三角形上面，否则三角内部的实心区域会把它整个盖掉
                *px = if p.x * p.x + y * y <= dot2 {
                    dot_color
                } else if inside_triangle(&pts, p) {
                    tri_color
                } else {
                    bg_color
                };
            }
        }

        display
            .draw_raw_slice(left, top, left + GEO_BOX - 1, top + GEO_BOX - 1, buf)
            .expect("draw failed");
        Timer::after(Duration::from_millis(10)).await;
    }
}

/// 效果二：两条相位相反的正弦波。
///
/// 每帧整块重算波形区再一次送出；相邻列之间用竖线插值，曲线不会断成点。
async fn effect_wave(display: &mut Display, buf: &mut [u16]) {
    info!("effect: sine wave");
    let top = (DISPLAY_H - WAVE_H) / 2;
    let mid = (WAVE_H / 2) as i32;
    let width = DISPLAY_W as usize;
    let bg = RawU16::from(Rgb565::BLACK).into_inner();
    let up_color = RawU16::from(Rgb565::CYAN).into_inner();
    let down_color = RawU16::from(Rgb565::YELLOW).into_inner();

    display.clear(Rgb565::BLACK).expect("clear failed");

    for frame in 0..WAVE_FRAMES {
        let phase = frame * WAVE_PHASE_STEP;
        buf.fill(bg);

        let mut prev_up = mid;
        let mut prev_down = mid;
        for x in 0..width {
            let amp = sin_q10(phase + x as u16 * WAVE_STEP) * WAVE_AMP / 1024;
            let up = mid + amp;
            let down = mid - amp;

            for y in prev_up.min(up)..=prev_up.max(up) {
                buf[y as usize * width + x] = up_color;
            }
            for y in prev_down.min(down)..=prev_down.max(down) {
                buf[y as usize * width + x] = down_color;
            }
            prev_up = up;
            prev_down = down;
        }

        display
            .draw_raw_slice(0, top, DISPLAY_W - 1, top + WAVE_H - 1, buf)
            .expect("draw failed");
        Timer::after(Duration::from_millis(20)).await;
    }
}

/// 一个弹跳小球。位置与速度都取 Q4 定点，慢速时也不会因取整卡住。
#[derive(Clone, Copy)]
struct Ball {
    /// 中心 x（Q4）。
    x: i32,
    /// 中心 y（Q4）。
    y: i32,
    /// 上一帧的中心 x（Q4），用来算需要重画的矩形。
    prev_x: i32,
    /// 上一帧的中心 y（Q4）。
    prev_y: i32,
    /// 每帧 x 位移（Q4）。
    vx: i32,
    /// 每帧 y 位移（Q4）。
    vy: i32,
}

/// 效果三：多个不同颜色的小球在屏幕内弹跳，碰到边缘反向。
///
/// 每帧只重画每个球「旧位置 ∪ 新位置」的小矩形：整块在内存里算好再一次性送出，
/// 没有先擦后画的中间态，也就不会闪。矩形内逐像素按所有球判定，互相遮挡时后画的压住先画的。
async fn effect_balls(display: &mut Display, buf: &mut [u16]) {
    info!("effect: bouncing balls");
    let bg = RawU16::from(Rgb565::BLACK).into_inner();
    let rq = BALL_R * BALL_ONE;
    let x_max = (DISPLAY_W as i32 - 1) * BALL_ONE - rq;
    let y_max = (DISPLAY_H as i32 - 1) * BALL_ONE - rq;
    let rr = BALL_R * BALL_R;

    let mut balls = [Ball {
        x: 0,
        y: 0,
        prev_x: 0,
        prev_y: 0,
        vx: 0,
        vy: 0,
    }; BALL_COUNT];
    for (ball, (x, y, vx, vy)) in balls.iter_mut().zip(BALLS_INIT) {
        *ball = Ball {
            x: x * BALL_ONE,
            y: y * BALL_ONE,
            prev_x: x * BALL_ONE,
            prev_y: y * BALL_ONE,
            vx,
            vy,
        };
    }

    display.clear(Rgb565::BLACK).expect("clear failed");

    for _ in 0..BALL_FRAMES {
        for b in balls.iter_mut() {
            b.prev_x = b.x;
            b.prev_y = b.y;
            b.x += b.vx;
            b.y += b.vy;

            if b.x < rq {
                b.x = rq;
                b.vx = b.vx.abs();
            } else if b.x > x_max {
                b.x = x_max;
                b.vx = -b.vx.abs();
            }
            if b.y < rq {
                b.y = rq;
                b.vy = b.vy.abs();
            } else if b.y > y_max {
                b.y = y_max;
                b.vy = -b.vy.abs();
            }
        }

        for i in 0..BALL_COUNT {
            let b = &balls[i];
            let old_cx = b.prev_x / BALL_ONE;
            let old_cy = b.prev_y / BALL_ONE;
            let cx = b.x / BALL_ONE;
            let cy = b.y / BALL_ONE;

            let x0 = (old_cx.min(cx) - BALL_R - 1).max(0);
            let x1 = (old_cx.max(cx) + BALL_R + 1).min(DISPLAY_W as i32 - 1);
            let y0 = (old_cy.min(cy) - BALL_R - 1).max(0);
            let y1 = (old_cy.max(cy) + BALL_R + 1).min(DISPLAY_H as i32 - 1);
            let bw = (x1 - x0 + 1) as usize;

            for py in y0..=y1 {
                for px in x0..=x1 {
                    let mut color = bg;
                    for (j, other) in balls.iter().enumerate() {
                        let dx = px - other.x / BALL_ONE;
                        let dy = py - other.y / BALL_ONE;
                        if dx * dx + dy * dy <= rr {
                            color = BALL_COLORS[j];
                            break;
                        }
                    }
                    buf[(py - y0) as usize * bw + (px - x0) as usize] = color;
                }
            }

            let bh = (y1 - y0 + 1) as usize;
            display
                .draw_raw_slice(x0 as u16, y0 as u16, x1 as u16, y1 as u16, &buf[..bw * bh])
                .expect("draw failed");
        }

        Timer::after(Duration::from_millis(10)).await;
    }
}

/// 三角波：相位 u8 映射到 5 位色深，避免逐像素算浮点三角函数。
const fn tri5(phase: u8) -> u16 {
    let v = if phase < 128 { phase } else { 255 - phase };
    (v >> 2) as u16
}

/// 效果四：逐像素算的等离子波。
async fn effect_plasma(display: &mut Display) {
    info!("effect: plasma");
    for frame in 0..PLASMA_FRAMES {
        let t = (frame * PLASMA_SPEED) as u8;
        let pixels = (0..u32::from(DISPLAY_W) * u32::from(DISPLAY_H)).map(move |i| {
            let x = (i % u32::from(DISPLAY_W)) as u8;
            let y = (i / u32::from(DISPLAY_W)) as u8;
            let r = tri5(x.wrapping_mul(3).wrapping_add(t));
            let g = tri5(
                y.wrapping_mul(2)
                    .wrapping_add(x.wrapping_mul(2))
                    .wrapping_sub(t),
            );
            let b = tri5(
                x.wrapping_add(y.wrapping_mul(3))
                    .wrapping_add(t.wrapping_mul(2)),
            );
            (r << 11) | (g << 5) | b
        });
        display
            .draw_raw_iter(0, 0, DISPLAY_W - 1, DISPLAY_H - 1, pixels)
            .expect("plasma failed");
        Timer::after(Duration::from_millis(10)).await;
    }
}

/// 把字幕的一帧渲染进缓冲：每列只查一次字形，再填满整列。
fn render_band(band: &mut [u16], shift: i32) {
    let text_len = SCROLL_TEXT.len() as i32;

    for x in 0..DISPLAY_W as usize {
        let px = (x as i32 + shift).rem_euclid(GLYPH_ADVANCE * text_len);
        let glyph =
            font8x8::legacy::BASIC_LEGACY[SCROLL_TEXT[(px / GLYPH_ADVANCE) as usize] as usize];
        let col = (px % GLYPH_ADVANCE) as usize / SCROLL_SCALE;

        for (row, bits) in glyph.iter().enumerate() {
            let color = if (bits >> col) & 1 == 1 {
                SCROLL_FG
            } else {
                SCROLL_BG
            };
            for dy in 0..SCROLL_SCALE {
                band[(row * SCROLL_SCALE + dy) * DISPLAY_W as usize + x] = color;
            }
        }
    }
}

/// 效果五：滚动字幕 + 底部进度条。
async fn effect_scroll_text(display: &mut Display, band: &mut [u16]) {
    info!("effect: scroll text");
    let top = (DISPLAY_H - BAND_H) / 2;
    const BAR: u16 = 260;
    /// 进度条左上角。
    const BAR_ORIGIN: (i32, i32) = (30, 200);
    /// 进度条高度。
    const BAR_H: u32 = 12;

    display.clear(Rgb565::BLACK).expect("clear failed");

    for frame in 0..SCROLL_FRAMES {
        render_band(band, frame as i32 * SCROLL_SPEED);
        display
            .draw_raw_slice(0, top, DISPLAY_W - 1, top + BAND_H - 1, band)
            .expect("text failed");

        let done = (frame as u32 + 1) * u32::from(BAR) / u32::from(SCROLL_FRAMES);
        Rectangle::new(
            Point::new(BAR_ORIGIN.0, BAR_ORIGIN.1),
            Size::new(done, BAR_H),
        )
        .into_styled(PrimitiveStyle::with_fill(Rgb565::YELLOW))
        .draw(display)
        .expect("bar failed");
        Rectangle::new(
            Point::new(BAR_ORIGIN.0 + done as i32, BAR_ORIGIN.1),
            Size::new(u32::from(BAR) - done, BAR_H),
        )
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(display)
        .expect("bar failed");

        Timer::after(Duration::from_millis(30)).await;
    }
}

/// 把一行汉字画进文字带缓冲：每个字形像素放大 `CJK_SCALE` 倍，只写点亮的部分。
fn render_cjk(buf: &mut [u16], text: &str, shown: usize, fg: u16) {
    let advance = font_cjk::GLYPH_SIZE * CJK_SCALE + CJK_GAP;
    let count = text.chars().count();
    let total = count * advance - CJK_GAP;
    let mut x = (DISPLAY_W as usize - total) / 2;

    for (i, ch) in text.chars().enumerate() {
        if i >= shown {
            break;
        }
        if let Some(glyph) = font_cjk::glyph(ch) {
            for row in 0..font_cjk::GLYPH_SIZE {
                let bits = u16::from_be_bytes([glyph[row * 2], glyph[row * 2 + 1]]);
                for col in 0..font_cjk::GLYPH_SIZE {
                    if bits & (1 << (15 - col)) == 0 {
                        continue;
                    }
                    for dy in 0..CJK_SCALE {
                        for dx in 0..CJK_SCALE {
                            let px = x + col * CJK_SCALE + dx;
                            let py = row * CJK_SCALE + dy;
                            buf[py * DISPLAY_W as usize + px] = fg;
                        }
                    }
                }
            }
        }
        x += advance;
    }
}

/// 效果六：GB2312 汉字逐字出现。
///
/// 字模是 Unifont 的 16×16 点阵（见 `tools/gen_cjk_font.py`），放大 2 倍后一行放得下 9 个字。
async fn effect_cjk(display: &mut Display, buf: &mut [u16]) {
    info!("effect: cjk text");
    let top = (DISPLAY_H - CJK_BAND_H) / 2;
    let bg = RawU16::from(Rgb565::BLACK).into_inner();
    let fg = RawU16::from(Rgb565::WHITE).into_inner();

    display.clear(Rgb565::BLACK).expect("clear failed");

    for shown in 1..=CJK_TEXT.chars().count() {
        buf.fill(bg);
        render_cjk(buf, CJK_TEXT, shown, fg);
        display
            .draw_raw_slice(0, top, DISPLAY_W - 1, top + CJK_BAND_H - 1, buf)
            .expect("draw failed");
        Timer::after(CJK_STEP).await;
    }

    Timer::after(CJK_HOLD).await;
}

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let mut delay = Delay::new();

    let data = [
        Output::new(peripherals.GPIO0, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO1, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO2, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO3, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO4, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO5, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO6, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO7, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO8, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO10, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO11, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO18, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO19, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO20, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO21, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO22, Level::Low, OutputConfig::default()),
    ];

    let bus = Bus::new(
        data,
        Output::new(peripherals.GPIO15, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO23, Level::High, OutputConfig::default()),
    );

    let mut display = Ili9341::new(
        bus,
        NoReset,
        &mut delay,
        Orientation::Landscape,
        DisplaySize240x320,
    )
    .expect("ili9341 init failed");
    info!("ili9341 ready {}x{}", display.width(), display.height());

    let geo_buf = GEO_FRAME.init([0; GEO_BOX as usize * GEO_BOX as usize]);
    let wave_buf = WAVE_FRAME.init([0; DISPLAY_W as usize * WAVE_H as usize]);
    let ball_buf = BALL_BUF.init([0; BALL_BOX * BALL_BOX]);
    let band = BAND.init([0; DISPLAY_W as usize * BAND_H as usize]);
    let cjk_buf = CJK_BAND.init([0; DISPLAY_W as usize * CJK_BAND_H as usize]);

    loop {
        effect_geometry(&mut display, &mut geo_buf[..]).await;
        effect_wave(&mut display, &mut wave_buf[..]).await;
        effect_balls(&mut display, &mut ball_buf[..]).await;
        effect_plasma(&mut display).await;
        effect_scroll_text(&mut display, &mut band[..]).await;
        effect_cjk(&mut display, &mut cjk_buf[..]).await;
    }
}
