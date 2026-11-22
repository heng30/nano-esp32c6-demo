//! 音乐播放器页：用 LVGL 自己的控件搭出来，而不是逐像素自己画。
//!
//! 唱片是一张 RAM 位图（[`DISC_PX`]）交给 `lv_img`，靠 `lv_img_set_angle` 整块旋转；
//! 唱臂是 `lv_line` 的两点连线；进度条是 `lv_bar`；歌词一列 `lv_label`，滚动靠逐帧改 y。
//!
//! 全部走 `lvgl-sys` 的原始 FFI：`lvgl 0.6` 的安全包装层没有 img/line/btn 这些控件。

// 这里从头到尾都是 FFI：`unsafe fn` 里再套一层 `unsafe {}` 只是噪音
#![allow(unsafe_op_in_unsafe_fn)]

use core::ptr::{addr_of, addr_of_mut, null_mut};

use lvgl_sys as lv;

/// 屏宽（横屏）。
pub const WIDTH: lv::lv_coord_t = 320;
/// 屏高。
pub const HEIGHT: lv::lv_coord_t = 240;

/// 唱片机基座（x, y, w, h）：盘子和唱臂都装在这块板上。
const PLINTH: (lv::lv_coord_t, lv::lv_coord_t, lv::lv_coord_t, lv::lv_coord_t) = (6, 6, 160, 146);
/// 基座底部那条暗边的厚度，用来做出板材的厚度感。
const PLINTH_LIP: lv::lv_coord_t = 6;

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

/// 全部歌词按播放顺序平铺；都是 C 字符串，直接交给 `lv_label_set_text`。
const LYRICS: [&[u8]; 15] = [
    b"neon over silicon\0",
    b"ride empty bus\0",
    b"static on radio\0",
    b"sings the city\0",
    b"night goes on\0",
    b"ghost in machine\0",
    b"counting stars\0",
    b"every frame beats\0",
    b"every pixel ours\0",
    b"dance on black\0",
    b"needle in groove\0",
    b"thirty-three rpm\0",
    b"turn night slow\0",
    b"hold low note\0",
    b"till morning\0",
];

/// 播放列表总时长（毫秒），由 [`SONGS`] 求和得来。
const CYCLE_MS: u32 = {
    let mut sum = 0;
    let mut i = 0;
    while i < SONGS.len() {
        sum += SONGS[i].ms;
        i += 1;
    }
    sum
};

/// 唱片位图边长（像素）。
const DISC: usize = 132;
/// 唱片在屏上的位置。
const DISC_X: lv::lv_coord_t = 12;
const DISC_Y: lv::lv_coord_t = 12;
/// 唱片半径。
const DISC_R: i32 = 62;
/// 中心标签半径。
const LABEL_R: i32 = 20;
/// 唱片纹路间隔（像素）。
const GROOVE_STEP: i32 = 4;
/// 唱臂支点，在盘外右上。
const PIVOT: (lv::lv_coord_t, lv::lv_coord_t) = (152, 16);
/// 唱针起点相对盘心的偏移：落在标签外沿（内环），盘心那段是标签不是纹路。
const NEEDLE_NEAR: (i32, i32) = (12, -16);
/// 唱针终点相对盘心的偏移：落在盘边内侧。
const NEEDLE_FAR: (i32, i32) = (36, -48);
/// 唱针头尺寸，比唱臂粗一点才看得出是「头」而不是线的末端。
const NEEDLE_W: lv::lv_coord_t = 9;
const NEEDLE_H: lv::lv_coord_t = 13;
/// 转速，33⅓ rpm ≈ 200°/s。
const DEG_PER_S: i32 = 200;
/// 进度条位置与尺寸。
const BAR_X: lv::lv_coord_t = 8;
const BAR_Y: lv::lv_coord_t = 156;
const BAR_W: lv::lv_coord_t = 158;
const BAR_H: lv::lv_coord_t = 10;
/// 三个按钮的顶边、尺寸与横向起点。
const BTN_Y: lv::lv_coord_t = 182;
const BTN_W: lv::lv_coord_t = 50;
const BTN_H: lv::lv_coord_t = 34;
const BTN_X0: lv::lv_coord_t = 8;
const BTN_GAP: lv::lv_coord_t = 4;
/// 歌词列：左边、行高、当前行在屏上的 y。
const LYRIC_X: lv::lv_coord_t = 178;
const LYRIC_W: lv::lv_coord_t = 138;
const LYRIC_STEP: i32 = 26;
const LYRIC_MID: i32 = 120;
/// 当前行上下各留几行可见（再多就藏掉，免得压到进度条）。
const LYRIC_SPAN: i32 = 3;

/// 唱片位图的配色。
const PX_DISC: u16 = 0x18c3;
const PX_GROOVE: u16 = 0x2945;
const PX_RIM: u16 = 0x52aa;
const PX_LABEL_A: u16 = 0xfb40;
const PX_LABEL_B: u16 = 0xc320;

/// 唱片位图每个像素的字节数：RGB565 + 透明度，盘外透明才能露出基座。
const DISC_BYTES: usize = DISC * DISC * 3;

/// 唱片位图与它的 LVGL 描述符，`lv_img` 只借用指针，必须活得够久。
static mut DISC_PX: [u8; DISC_BYTES] = [0; DISC_BYTES];
static mut DISC_DSC: core::mem::MaybeUninit<lv::lv_img_dsc_t> = core::mem::MaybeUninit::uninit();
/// 唱臂的两个点（支点、唱针），原地改完再交给 `lv_line`。
static mut ARM_PTS: [lv::lv_point_t; 2] = [lv::lv_point_t { x: 0, y: 0 }; 2];

/// 音乐播放器页的全部控件句柄。
pub struct Music {
    /// 唱片位图控件。
    disc: *mut lv::lv_obj_t,
    /// 唱臂线段。
    arm: *mut lv::lv_obj_t,
    /// 唱针头，跟着线段末端跑。
    needle: *mut lv::lv_obj_t,
    /// 进度条。
    bar: *mut lv::lv_obj_t,
    /// 全部歌词标签。
    lyrics: [*mut lv::lv_obj_t; LYRICS.len()],
    /// 歌词滚动位置，Q8 定点：整数部分是「第几行」。
    scroll: i32,
}

impl Music {
    /// 在当前活动屏幕上建好整页控件。
    pub fn build() -> Self {
        unsafe {
            let scr = lv::lv_disp_get_scr_act(lv::lv_disp_get_default());
            lv::lv_obj_set_style_bg_color(scr, rgb(0, 0, 0), lv::LV_PART_MAIN);

            Self::build_plinth(scr);
            let disc = Self::build_disc(scr);
            let arm = Self::build_arm(scr);
            let needle = Self::build_needle(scr);
            let bar = Self::build_bar(scr);
            Self::build_buttons(scr);
            let lyrics = Self::build_lyrics(scr);

            Self {
                disc,
                arm,
                needle,
                bar,
                lyrics,
                scroll: 0,
            }
        }
    }

    /// 把 t_ms 时刻的状态推给 LVGL：转盘角度、唱针位置、进度、歌词行。
    pub fn tick(&mut self, t_ms: u32) {
        let (song, pos) = locate(t_ms);
        let progress = (pos * 1000 / SONGS[song].ms) as i32;

        unsafe {
            // 唱片整块旋转：LVGL 的角度单位是 0.1°，自己取模避免溢出 i16
            let angle = ((t_ms as i32) * DEG_PER_S / 1000 % 3600) as i16;
            lv::lv_img_set_angle(self.disc, angle);

            let (nx, ny) = update_arm(progress);
            lv::lv_line_set_points(self.arm, addr_of!(ARM_PTS).cast(), 2);
            lv::lv_obj_set_pos(self.needle, nx - NEEDLE_W / 2, ny - NEEDLE_H / 2);

            lv::lv_bar_set_value(self.bar, progress, lv_anim_off());

            self.tick_lyrics(song, pos);
        }
    }

    /// 歌词滚动：目标行号一阶低通，滚过去而不是跳行。
    unsafe fn tick_lyrics(&mut self, song: usize, pos: u32) {
        let count = SONGS[song].lines;
        let line = (pos as usize * count / SONGS[song].ms as usize).min(count - 1);
        let target = ((SONGS[song].first + line) as i32) << 8;

        self.scroll += (target - self.scroll) / 3;
        if (target - self.scroll).abs() < 16 {
            self.scroll = target;
        }

        for (i, label) in self.lyrics.iter().enumerate() {
            let delta = (i as i32 * 256 - self.scroll).div_euclid(256);
            if delta.abs() > LYRIC_SPAN {
                lv::lv_obj_add_flag(*label, lv::LV_OBJ_FLAG_HIDDEN);
                continue;
            }
            lv::lv_obj_clear_flag(*label, lv::LV_OBJ_FLAG_HIDDEN);
            let y = LYRIC_MID + delta * LYRIC_STEP;
            lv::lv_obj_set_pos(*label, LYRIC_X, y as lv::lv_coord_t);

            let color = if delta == 0 {
                rgb(255, 255, 255)
            } else {
                rgb(92, 107, 122)
            };
            lv::lv_obj_set_style_text_color(*label, color, lv::LV_PART_MAIN);
        }
    }

    /// 生成唱片位图并挂到 `lv_img` 上。
    unsafe fn build_disc(scr: *mut lv::lv_obj_t) -> *mut lv::lv_obj_t {
        draw_disc();

        let dsc = addr_of_mut!(DISC_DSC).cast::<lv::lv_img_dsc_t>();
        core::ptr::write(dsc, core::mem::zeroed());
        (*dsc).header.set_cf(lv::LV_IMG_CF_TRUE_COLOR_ALPHA);
        (*dsc).header.set_w(DISC as u32);
        (*dsc).header.set_h(DISC as u32);
        (*dsc).data_size = DISC_BYTES as u32;
        (*dsc).data = addr_of!(DISC_PX).cast::<u8>();

        let disc = lv::lv_img_create(scr);
        lv::lv_img_set_src(disc, dsc.cast::<core::ffi::c_void>());
        lv::lv_obj_set_pos(disc, DISC_X, DISC_Y);
        // 支点必须在转起来之前定在盘心，否则会绕左上角转
        lv::lv_img_set_pivot(
            disc,
            (DISC / 2) as lv::lv_coord_t,
            (DISC / 2) as lv::lv_coord_t,
        );
        disc
    }

    /// 唱片机基座：上亮下暗的竖向渐变，顶部压一条高光、底部压一条暗边，
    /// 看起来是一块有厚度的板而不是一个色块。
    unsafe fn build_plinth(scr: *mut lv::lv_obj_t) {
        let (x, y, w, h) = PLINTH;

        let plinth = lv::lv_obj_create(scr);
        lv::lv_obj_set_pos(plinth, x, y);
        lv::lv_obj_set_size(plinth, w, h);
        lv::lv_obj_set_style_radius(plinth, 8, lv::LV_PART_MAIN);
        lv::lv_obj_set_style_border_width(plinth, 0, lv::LV_PART_MAIN);
        lv::lv_obj_set_style_bg_color(plinth, rgb(42, 47, 54), lv::LV_PART_MAIN);
        lv::lv_obj_set_style_bg_grad_color(plinth, rgb(16, 18, 22), lv::LV_PART_MAIN);
        lv::lv_obj_set_style_bg_grad_dir(plinth, lv::LV_GRAD_DIR_VER as u8, lv::LV_PART_MAIN);

        // 顶面高光
        let edge = lv::lv_obj_create(scr);
        lv::lv_obj_set_pos(edge, x + 4, y + 2);
        lv::lv_obj_set_size(edge, w - 8, 2);
        lv::lv_obj_set_style_bg_color(edge, rgb(78, 86, 98), lv::LV_PART_MAIN);
        lv::lv_obj_set_style_border_width(edge, 0, lv::LV_PART_MAIN);
        lv::lv_obj_set_style_radius(edge, 1, lv::LV_PART_MAIN);

        // 底部暗边：板厚
        let lip = lv::lv_obj_create(scr);
        lv::lv_obj_set_pos(lip, x + 2, y + h - PLINTH_LIP);
        lv::lv_obj_set_size(lip, w - 4, PLINTH_LIP - 1);
        lv::lv_obj_set_style_bg_color(lip, rgb(8, 9, 12), lv::LV_PART_MAIN);
        lv::lv_obj_set_style_border_width(lip, 0, lv::LV_PART_MAIN);
        lv::lv_obj_set_style_radius(lip, 0, lv::LV_PART_MAIN);
    }

    /// 唱臂线段：两点由 [`update_arm`] 每帧改写。
    unsafe fn build_arm(scr: *mut lv::lv_obj_t) -> *mut lv::lv_obj_t {
        let arm = lv::lv_line_create(scr);
        lv::lv_obj_set_pos(arm, 0, 0);
        lv::lv_obj_set_size(arm, WIDTH, HEIGHT);
        lv::lv_obj_set_style_line_width(arm, 3, lv::LV_PART_MAIN);
        lv::lv_obj_set_style_line_color(arm, rgb(198, 198, 198), lv::LV_PART_MAIN);

        let hub = lv::lv_obj_create(scr);
        lv::lv_obj_set_pos(hub, PIVOT.0 - 5, PIVOT.1 - 5);
        lv::lv_obj_set_size(hub, 10, 10);
        lv::lv_obj_set_style_bg_color(hub, rgb(198, 198, 198), lv::LV_PART_MAIN);
        lv::lv_obj_set_style_border_width(hub, 0, lv::LV_PART_MAIN);
        lv::lv_obj_set_style_radius(hub, lv::LV_RADIUS_CIRCLE as lv::lv_coord_t, lv::LV_PART_MAIN);
        arm
    }

    /// 唱针头：线段末端的小方块，位置由 [`Music::tick`] 每帧改。
    unsafe fn build_needle(scr: *mut lv::lv_obj_t) -> *mut lv::lv_obj_t {
        let needle = lv::lv_obj_create(scr);
        lv::lv_obj_set_pos(needle, 0, 0);
        lv::lv_obj_set_size(needle, NEEDLE_W, NEEDLE_H);
        lv::lv_obj_set_style_bg_color(needle, rgb(214, 214, 214), lv::LV_PART_MAIN);
        lv::lv_obj_set_style_border_width(needle, 0, lv::LV_PART_MAIN);
        lv::lv_obj_set_style_radius(needle, 2, lv::LV_PART_MAIN);
        needle
    }

    /// 进度条：轨道深灰、已播橙色。
    unsafe fn build_bar(scr: *mut lv::lv_obj_t) -> *mut lv::lv_obj_t {
        let bar = lv::lv_bar_create(scr);
        lv::lv_obj_set_pos(bar, BAR_X, BAR_Y);
        lv::lv_obj_set_size(bar, BAR_W, BAR_H);
        lv::lv_bar_set_range(bar, 0, 1000);
        lv::lv_obj_set_style_bg_color(bar, rgb(72, 82, 92), lv::LV_PART_MAIN);
        lv::lv_obj_set_style_bg_color(bar, rgb(252, 152, 0), lv::LV_PART_INDICATOR);
        lv::lv_obj_set_style_radius(bar, 0, lv::LV_PART_MAIN);
        bar
    }

    /// 上一首 / 暂停 / 下一首：屏没有触摸，纯装饰。
    unsafe fn build_buttons(scr: *mut lv::lv_obj_t) {
        let symbols = [lv::LV_SYMBOL_PREV, lv::LV_SYMBOL_PAUSE, lv::LV_SYMBOL_NEXT];
        for (i, symbol) in symbols.iter().enumerate() {
            let btn = lv::lv_btn_create(scr);
            lv::lv_obj_set_pos(btn, BTN_X0 + i as lv::lv_coord_t * (BTN_W + BTN_GAP), BTN_Y);
            lv::lv_obj_set_size(btn, BTN_W, BTN_H);
            lv::lv_obj_set_style_bg_color(btn, rgb(38, 44, 52), lv::LV_PART_MAIN);
            lv::lv_obj_set_style_border_width(btn, 0, lv::LV_PART_MAIN);
            lv::lv_obj_set_style_radius(btn, 4, lv::LV_PART_MAIN);

            let label = lv::lv_label_create(btn);
            lv::lv_label_set_text(label, symbol.as_ptr().cast::<core::ffi::c_char>());
            lv::lv_obj_align(label, lv::LV_ALIGN_CENTER as u8, 0, 0);
        }
    }

    /// 一列歌词标签，位置与颜色由 [`Music::tick_lyrics`] 每帧改。
    unsafe fn build_lyrics(scr: *mut lv::lv_obj_t) -> [*mut lv::lv_obj_t; LYRICS.len()] {
        let mut labels = [null_mut(); LYRICS.len()];
        for (i, line) in LYRICS.iter().enumerate() {
            let label = lv::lv_label_create(scr);
            lv::lv_obj_set_pos(label, LYRIC_X, LYRIC_MID as lv::lv_coord_t);
            // 必须固定成单行高度：默认会按内容换行，两行歌词就会叠在一起
            lv::lv_obj_set_size(label, LYRIC_W, LYRIC_STEP as lv::lv_coord_t - 4);
            lv::lv_label_set_long_mode(label, lv::LV_LABEL_LONG_DOT as lv::lv_label_long_mode_t);
            lv::lv_label_set_text(label, line.as_ptr().cast::<core::ffi::c_char>());
            lv::lv_obj_set_style_text_color(label, rgb(92, 107, 122), lv::LV_PART_MAIN);
            labels[i] = label;
        }
        labels
    }
}

/// 每首歌曲的进度换算成唱针坐标：写进 [`ARM_PTS`]，并把针头中心返回给调用方。
///
/// 行程从标签外沿（内环）到盘边：进度 0 时针头正好在纹路最内侧。
fn update_arm(progress: i32) -> (lv::lv_coord_t, lv::lv_coord_t) {
    let (cx, cy) = (DISC_X as i32 + DISC_R, DISC_Y as i32 + DISC_R);
    let (nx, ny) = (
        (cx + NEEDLE_NEAR.0 + (NEEDLE_FAR.0 - NEEDLE_NEAR.0) * progress / 1000) as lv::lv_coord_t,
        (cy + NEEDLE_NEAR.1 + (NEEDLE_FAR.1 - NEEDLE_NEAR.1) * progress / 1000) as lv::lv_coord_t,
    );
    let pts = unsafe { &mut *addr_of_mut!(ARM_PTS) };
    pts[0] = lv::lv_point_t {
        x: PIVOT.0,
        y: PIVOT.1,
    };
    pts[1] = lv::lv_point_t { x: nx, y: ny };
    (nx, ny)
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

/// LVGL 8 的 `lv_bar_set_value` 第三个参数：0 = 不做补间，我们自己按帧推。
fn lv_anim_off() -> lv::lv_anim_enable_t {
    0
}

/// RGB 三元组转 LVGL 颜色。
fn rgb(r: u8, g: u8, b: u8) -> lv::lv_color_t {
    unsafe { lv::_LV_COLOR_MAKE(r, g, b) }
}

/// 一次性画出唱片位图：盘面 + 同心纹路 + 四象限标签，旋转交给 LVGL。
///
/// 每个像素写 RGB565 + 透明度三字节：盘外与轴孔透明，基座的渐变才透得上来。
fn draw_disc() {
    let px = unsafe { &mut *addr_of_mut!(DISC_PX) };
    let center = DISC as i32 / 2;

    for y in 0..DISC as i32 {
        for x in 0..DISC as i32 {
            let (dx, dy) = (x - center, y - center);
            let d2 = dx * dx + dy * dy;
            let color = if d2 > DISC_R * DISC_R || d2 <= 9 {
                None
            } else if d2 <= LABEL_R * LABEL_R {
                Some(if (dx > 0) == (dy > 0) {
                    PX_LABEL_A
                } else {
                    PX_LABEL_B
                })
            } else if d2 > (DISC_R - 2) * (DISC_R - 2) {
                Some(PX_RIM)
            } else {
                // 纹路用半径平方分档：比开方省事，越靠外越密正好是唱片的观感
                let band = d2 / (LABEL_R * LABEL_R * GROOVE_STEP / 6);
                Some(if band % 2 == 0 { PX_GROOVE } else { PX_DISC })
            };

            let at = (y as usize * DISC + x as usize) * 3;
            match color {
                Some(c) => {
                    px[at] = c as u8;
                    px[at + 1] = (c >> 8) as u8;
                    px[at + 2] = 0xff;
                }
                None => {
                    px[at] = 0;
                    px[at + 1] = 0;
                    px[at + 2] = 0;
                }
            }
        }
    }
}
