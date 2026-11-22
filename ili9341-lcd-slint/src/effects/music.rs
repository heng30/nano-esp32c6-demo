//! 音乐播放器：黑胶唱片旋转、唱针从盘心慢慢移向盘边，底部是进度条和上一首/暂停/下一首。
//!
//! 歌词交给 Slint 画（Rust 侧没有字形），排版在 `ui/effects/music.slint`，
//! 这里只在换行时把行号推过去。播放列表全是假的，按时间轴自己走。

use alloc::vec::Vec;

use slint::platform::software_renderer::Rgb565Pixel;
use slint::{ModelRc, SharedString, VecModel};

use super::{Canvas, HEIGHT, WIDTH, sin_q10};
use crate::MainWindow;

/// 一首歌：时长与它的歌词在 [`LYRICS`] 里的位置。
struct Song {
    /// 时长，毫秒。
    ms: u32,
    /// 首行在 [`LYRICS`] 里的下标。
    first: usize,
    /// 歌词行数。
    lines: usize,
}

/// 播放列表，走完 [`CYCLE_MS`] 后回到第一首。
const SONGS: [Song; 3] = [
    Song {
        ms: 12_000,
        first: 0,
        lines: 5,
    },
    Song {
        ms: 9_000,
        first: 5,
        lines: 4,
    },
    Song {
        ms: 15_000,
        first: 9,
        lines: 6,
    },
];

/// 全部歌词按播放顺序平铺，换歌不用重设 Slint 的列表，只改当前行号。
const LYRICS: [&str; 15] = [
    "Neon over silicon",
    "ride the empty bus",
    "static on the radio",
    "sings the city back",
    "night goes on and on",
    "Ghost in the machine",
    "counting down stars",
    "every frame a beat",
    "every pixel is ours",
    "Dance on a black disc",
    "needle finds grooves",
    "spinning thirty-three",
    "turn the night slow",
    "hold the low note",
    "till morning comes",
];

/// 播放列表总时长（毫秒），由 [`SONGS`] 求和得来，改时长不用改这里。
const CYCLE_MS: u32 = {
    let mut sum = 0;
    let mut i = 0;
    while i < SONGS.len() {
        sum += SONGS[i].ms;
        i += 1;
    }
    sum
};

/// 页面底色。
const BG: u16 = 0x0000;
/// 盘面。
const DISC: u16 = 0x1082;
/// 盘缘轮辋，比盘面亮一档。
const RIM: u16 = 0x39e7;
/// 盘面高光，跟着盘转，用来看出转速。
const SHEEN: u16 = 0x4a49;
/// 标签两块交替色，四片扇区就是靠它转起来的。
const LABEL_A: u16 = 0xfb40;
const LABEL_B: u16 = 0xc320;
/// 唱臂、唱头。
const ARM: u16 = 0xc618;
/// 进度条轨道与已播部分。
const TRACK: u16 = 0x528a;
const PLAYED: u16 = 0xfd20;
/// 按钮图标。
const ICON: u16 = 0xc618;

/// 唱片圆心 x（画布坐标）。
const CX: i32 = 40;
/// 唱片圆心 y。
const CY: i32 = 52;
/// 唱片半径。
const R: i32 = 38;
/// 标签半径。
const LABEL_R: i32 = 13;
/// 中心轴孔半径的平方。
const HOLE2: i32 = 9;
/// 唱臂支点 x，在盘外右上方；不能越过 x=88，否则会跑到歌词区下面。
const PIVOT_X: i32 = 84;
/// 唱臂支点 y。
const PIVOT_Y: i32 = 10;
/// 唱针终点相对盘心的偏移：落在盘边内侧，方向右上。
const NEEDLE_DX: i32 = 23;
const NEEDLE_DY: i32 = -30;
/// 转速，33⅓ rpm ≈ 200°/s。
const DEG_PER_S: i32 = 200;

/// 进度条左边、上边、宽、高。
const BAR_X: i32 = 10;
const BAR_Y: i32 = 104;
const BAR_W: i32 = 140;
const BAR_H: i32 = 4;
/// 按钮图标顶边与高度。
const BTN_Y: i32 = 110;
const BTN_H: i32 = 8;

/// 音乐播放器页。
pub struct Music {
    /// 歌词列表是否已经推给 Slint（只推一次，之后只改行号）。
    sent: bool,
}

impl Music {
    pub fn new() -> Self {
        Self { sent: false }
    }
}

impl Canvas for Music {
    fn draw(&mut self, px: &mut [Rgb565Pixel], t_ms: u32) {
        px.fill(Rgb565Pixel(BG));

        let (song, pos) = locate(t_ms);
        // 本首播放进度，千分比：进度条和唱针共用
        let progress = (pos * 1000 / SONGS[song].ms) as i32;

        // 盘面：轮辋 + 深色本体，高光与标签用逆旋转后的坐标采样，整块盘就转起来了
        let rot = (t_ms as i32) * DEG_PER_S / 1000;
        let (s, c) = (sin_q10(rot), sin_q10(rot + 90));
        for y in CY - R..=CY + R {
            for x in CX - R..=CX + R {
                let (dx, dy) = (x - CX, y - CY);
                let d2 = dx * dx + dy * dy;
                if d2 > R * R {
                    continue;
                }
                let color = if d2 > (R - 1) * (R - 1) {
                    RIM
                } else if d2 <= HOLE2 {
                    BG
                } else if d2 <= LABEL_R * LABEL_R {
                    label_color(dx, dy, s, c)
                } else {
                    let (rx, ry) = rotate(dx, dy, s, c);
                    // 盘面纹路是对称的，靠两团相对的高光才看得出在转：以 ±26 为心的椭圆
                    let da = rx.abs() - 26;
                    if da * da + 4 * ry * ry < 64 {
                        SHEEN
                    } else {
                        DISC
                    }
                };
                set(px, x, y, color);
            }
        }

        // 唱臂：支点固定，唱针随进度从盘心滑向盘边
        let (nx, ny) = (
            CX + NEEDLE_DX * progress / 1000,
            CY + NEEDLE_DY * progress / 1000,
        );
        line(px, PIVOT_X, PIVOT_Y, nx, ny, ARM);
        fill_rect(px, PIVOT_X - 3, PIVOT_Y - 3, 7, 7, RIM);
        fill_rect(px, nx - 1, ny - 2, 3, 5, ARM);

        // 进度条 + 拖动点
        fill_rect(px, BAR_X, BAR_Y, BAR_W, BAR_H, TRACK);
        fill_rect(px, BAR_X, BAR_Y, BAR_W * progress / 1000, BAR_H, PLAYED);

        // 上一首 / 暂停 / 下一首（屏没有触摸，图标只是装饰）
        fill_rect(px, 52, BTN_Y, 2, BTN_H, ICON);
        arrow(px, 57, BTN_Y, -1);
        fill_rect(px, 78, BTN_Y, 2, BTN_H, ICON);
        fill_rect(px, 82, BTN_Y, 2, BTN_H, ICON);
        arrow(px, 94, BTN_Y, 1);
        fill_rect(px, 100, BTN_Y, 2, BTN_H, ICON);
    }

    fn sync(&mut self, ui: &MainWindow, t_ms: u32) {
        if !self.sent {
            self.sent = true;
            let lines: Vec<SharedString> = LYRICS
                .iter()
                .map(|line| SharedString::from(*line))
                .collect();
            ui.set_lyrics(ModelRc::new(VecModel::from(lines)));
        }

        let (song, pos) = locate(t_ms);
        let count = SONGS[song].lines;
        let line = (pos as usize * count / SONGS[song].ms as usize).min(count - 1);
        ui.set_lyrics_index((SONGS[song].first + line) as i32);
    }
}

/// 把播放时长换算成（第几首，本首已播毫秒）。
fn locate(t_ms: u32) -> (usize, u32) {
    let mut pos = t_ms % CYCLE_MS;
    for (i, song) in SONGS.iter().enumerate() {
        if pos < song.ms {
            return (i, pos);
        }
        pos -= song.ms;
    }
    (SONGS.len() - 1, 0)
}

/// 把相对盘心的偏移按 `-rot` 转回去，用来在「转正」的坐标系里采样静态图案。
///
/// `s` / `c` 是旋转角的正弦、余弦（Q10），返回像素坐标。
fn rotate(dx: i32, dy: i32, s: i32, c: i32) -> (i32, i32) {
    ((dx * c + dy * s) >> 10, (dy * c - dx * s) >> 10)
}

/// 标签配色：按转正后的象限分四片，两块颜色交替。
fn label_color(dx: i32, dy: i32, s: i32, c: i32) -> u16 {
    let (rx, ry) = rotate(dx, dy, s, c);
    if (rx > 0) == (ry > 0) {
        LABEL_A
    } else {
        LABEL_B
    }
}

/// 写一个像素，越界丢掉。
fn set(px: &mut [Rgb565Pixel], x: i32, y: i32, color: u16) {
    if x >= 0 && y >= 0 && x < WIDTH as i32 && y < HEIGHT as i32 {
        px[y as usize * WIDTH + x as usize] = Rgb565Pixel(color);
    }
}

/// 填充矩形，越界部分剪掉。
fn fill_rect(px: &mut [Rgb565Pixel], x: i32, y: i32, w: i32, h: i32, color: u16) {
    for py in y..y + h {
        for px_x in x..x + w {
            set(px, px_x, py, color);
        }
    }
}

/// 画一条 1 像素宽的线段，按较长的一轴等分步进。
fn line(px: &mut [Rgb565Pixel], x0: i32, y0: i32, x1: i32, y1: i32, color: u16) {
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
    for i in 0..=steps {
        set(
            px,
            x0 + (x1 - x0) * i / steps,
            y0 + (y1 - y0) * i / steps,
            color,
        );
    }
}

/// 画一个高 [`BTN_H`] 的三角形：竖边在 `x`，锥尖朝 `dir`（1 右、-1 左）。
fn arrow(px: &mut [Rgb565Pixel], x: i32, y: i32, dir: i32) {
    for row in 0..BTN_H {
        // 每行宽度 1,2,3,4,3,2,1 —— 竖边对齐，锥尖在最宽那行的末端
        let w = BTN_H / 2 + 1 - (row - BTN_H / 2).abs();
        for k in 0..w {
            set(px, x + dir * k, y + row, ICON);
        }
    }
}
