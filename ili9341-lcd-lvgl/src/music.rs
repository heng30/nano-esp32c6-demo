//! 音乐播放器页：用 LVGL 9 自己的控件搭出来，而不是逐像素自己画。
//!
//! 唱片是一张 RAM 位图（[`DISC_PX`]）交给 `lv_image`，靠 `lv_image_set_rotation` 整块旋转；
//! 唱臂是 `lv_line` 的两点连线；进度条是 `lv_bar`；歌词一列 `lv_label`，滚动靠逐帧改 y。
//!
//! 控件在建好后只留 4 字节的 [`Handle`]：`lvgl-rust` 的视图借用 session，存不进结构体；
//! 每帧在 [`Music::tick`] 里用 `get_mut(&mut lv)` 重新解析。

use core::ffi::CStr;
use core::mem::MaybeUninit;
use core::ptr::{addr_of, addr_of_mut};

use lvgl_rust::{
    Error, Handle, ImageColorkey, ImageData, ImageDsc, ImageSrc, Lvgl, Obj, Owned, PixelFormat,
    Selector, WidgetExt,
    color::{hex, rgb},
    enums::{Align, GradDir, LabelLongMode, Part},
    sys,
    widgets::{Bar, Button, Image, Label, Line},
};

/// 屏宽（横屏）。
pub const WIDTH: u32 = 320;
/// 屏高。
pub const HEIGHT: u32 = 240;

/// 唱片机基座（x, y, w, h）：盘子和唱臂都装在这块板上。
const PLINTH: (i32, i32, i32, i32) = (6, 6, 160, 146);
/// 基座底部那条暗边的厚度，用来做出板材的厚度感。
const PLINTH_LIP: i32 = 6;

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

/// 全部歌词按播放顺序平铺；都是 C 字符串，直接交给 `lv_label_set_text_static`。
const LYRICS: [&CStr; 15] = [
    c"neon over silicon",
    c"ride empty bus",
    c"static on radio",
    c"sings the city",
    c"night goes on",
    c"ghost in machine",
    c"counting stars",
    c"every frame beats",
    c"every pixel ours",
    c"dance on black",
    c"needle in groove",
    c"thirty-three rpm",
    c"turn night slow",
    c"hold low note",
    c"till morning",
];

/// 播放列表总时长（毫秒），由 [`SONGS`] 求和得来。
pub const CYCLE_MS: u32 = {
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
const DISC_X: i32 = 12;
const DISC_Y: i32 = 12;
/// 唱片半径。
const DISC_R: i32 = 62;
/// 中心标签半径。
const LABEL_R: i32 = 20;
/// 唱片纹路间隔（像素）。
const GROOVE_STEP: i32 = 4;
/// 唱臂支点，在盘外右上。
const PIVOT: (i32, i32) = (152, 16);
/// 唱针起点相对盘心的偏移：落在标签外沿（内环），盘心那段是标签不是纹路。
const NEEDLE_NEAR: (i32, i32) = (12, -16);
/// 唱针终点相对盘心的偏移：落在盘边内侧。
const NEEDLE_FAR: (i32, i32) = (36, -48);
/// 唱针头尺寸，比唱臂粗一点才看得出是「头」而不是线的末端。
const NEEDLE_W: i32 = 9;
const NEEDLE_H: i32 = 13;
/// 转速，33⅓ rpm ≈ 200°/s。
const DEG_PER_S: i32 = 200;
/// 进度条位置与尺寸。
const BAR_X: i32 = 8;
const BAR_Y: i32 = 156;
const BAR_W: i32 = 158;
const BAR_H: i32 = 10;
/// 三个按钮的顶边、尺寸、横向起点与间距。
const BTN_Y: i32 = 182;
const BTN_W: i32 = 50;
const BTN_H: i32 = 34;
const BTN_X0: i32 = 8;
const BTN_GAP: i32 = 4;
/// 歌词列：左边、行高、当前行在屏上的 y。
const LYRIC_X: i32 = 178;
const LYRIC_W: i32 = 138;
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
/// 盘外画成这个颜色，再由 `image_colorkey` 抠掉：LVGL 9 的软件渲染器还不支持 3 字节的
/// `ARGB8565`，硬边圆盘用 colorkey 比走 alpha 通道省一半内存（RGB565 两字节一像素）。
const PX_KEY: u16 = 0x0000;

/// 唱片位图每个像素的字节数：RGB565。
const DISC_BYTES: usize = DISC * DISC * 2;

/// 唱片位图的透明色。
static KEY: ImageColorkey = ImageColorkey::new(rgb(0, 0, 0), rgb(0, 0, 0));
/// 唱片位图，`lv_image` 只借用指针，必须活得够久。
static mut DISC_PX: ImageData<DISC_BYTES> = ImageData([0; DISC_BYTES]);
/// 位图描述符，`draw_disc` 里才填。
static mut DISC_DSC: MaybeUninit<ImageDsc> = MaybeUninit::uninit();
/// 唱臂的两个点（支点、唱针）：`lv_line` 只保留指针，原地改完再交给它。
static mut ARM_PTS: [sys::lv_point_precise_t; 2] = [sys::lv_point_precise_t { x: 0, y: 0 }; 2];

/// 音乐播放器页的动态控件句柄。
pub struct Music {
    /// 唱片位图控件。
    disc: Handle<Image>,
    /// 唱臂线段。
    arm: Handle<Line>,
    /// 唱针头，跟着线段末端跑。
    needle: Handle<Obj>,
    /// 进度条。
    bar: Handle<Bar>,
    /// 全部歌词标签。
    lyrics: [Handle<Label>; LYRICS.len()],
    /// 歌词滚动位置，Q8 定点：整数部分是「第几行」。
    scroll: i32,
    /// 这页自己的屏幕：没有父对象，必须由这里持有，否则 drop 时连屏幕一起删掉。
    _owned: Owned<Obj>,
    /// 同一个屏幕的句柄，切页时交给 `Lvgl::load_screen`。
    pub screen: Handle<Obj>,
}

impl Music {
    /// 在当前活动屏幕上建好整页控件。
    pub fn build(lv: &mut Lvgl) -> Result<Self, Error> {
        let owned = lv.create_screen()?;
        let screen = owned.get(lv)?.handle()?;

        let (plinth, edge, lip, disc, arm, hub, needle, bar, buttons, lyrics) = {
            let mut scr = owned.get_mut(lv)?;

            let plinth = Obj::create(&mut scr)?.handle()?;
            let edge = Obj::create(&mut scr)?.handle()?;
            let lip = Obj::create(&mut scr)?.handle()?;
            let disc = Image::create(&mut scr)?.handle()?;
            let arm = Line::create(&mut scr)?.handle()?;
            let hub = Obj::create(&mut scr)?.handle()?;
            let needle = Obj::create(&mut scr)?.handle()?;
            let bar = Bar::create(&mut scr)?.handle()?;

            let first = Button::create(&mut scr)?.handle()?;
            let mut buttons = [first; 3];
            for slot in buttons.iter_mut().skip(1) {
                *slot = Button::create(&mut scr)?.handle()?;
            }

            let first = Label::create(&mut scr)?.handle()?;
            let mut lyrics = [first; LYRICS.len()];
            for slot in lyrics.iter_mut().skip(1) {
                *slot = Label::create(&mut scr)?.handle()?;
            }

            (
                plinth, edge, lip, disc, arm, hub, needle, bar, buttons, lyrics,
            )
        };

        // 基座：上亮下暗的竖向渐变，顶部压一条高光、底部压一条暗边，看起来是一块有厚度的板。
        let (x, y, w, h) = PLINTH;
        {
            let mut o = plinth.get_mut(lv)?;
            o.set_pos(x, y);
            o.set_size(w, h);
            o.set_style_radius(8, Selector::DEFAULT);
            o.set_style_border_width(0, Selector::DEFAULT);
            o.set_style_bg_color(hex(0x2a2f36), Selector::DEFAULT);
            o.set_style_bg_grad_color(hex(0x101216), Selector::DEFAULT);
            o.set_style_bg_grad_dir(GradDir::VER, Selector::DEFAULT);
        }
        {
            let mut o = edge.get_mut(lv)?;
            o.set_pos(x + 4, y + 2);
            o.set_size(w - 8, 2);
            o.set_style_bg_color(hex(0x4e5662), Selector::DEFAULT);
            o.set_style_border_width(0, Selector::DEFAULT);
            o.set_style_radius(1, Selector::DEFAULT);
        }
        {
            let mut o = lip.get_mut(lv)?;
            o.set_pos(x + 2, y + h - PLINTH_LIP);
            o.set_size(w - 4, PLINTH_LIP - 1);
            o.set_style_bg_color(hex(0x08090c), Selector::DEFAULT);
            o.set_style_border_width(0, Selector::DEFAULT);
            o.set_style_radius(0, Selector::DEFAULT);
        }
        {
            let mut o = hub.get_mut(lv)?;
            o.set_pos(PIVOT.0 - 5, PIVOT.1 - 5);
            o.set_size(10, 10);
            o.set_style_bg_color(hex(0xc6c6c6), Selector::DEFAULT);
            o.set_style_border_width(0, Selector::DEFAULT);
            o.set_style_radius(sys::LV_RADIUS_CIRCLE as i32, Selector::DEFAULT);
        }
        {
            let mut o = needle.get_mut(lv)?;
            o.set_pos(0, 0);
            o.set_size(NEEDLE_W, NEEDLE_H);
            o.set_style_bg_color(hex(0xd6d6d6), Selector::DEFAULT);
            o.set_style_border_width(0, Selector::DEFAULT);
            o.set_style_radius(2, Selector::DEFAULT);
        }

        // 唱臂线段：两点每帧由 [`update_arm`] 改写。
        {
            let mut o = arm.get_mut(lv)?;
            o.set_pos(0, 0);
            o.set_size(WIDTH as i32, HEIGHT as i32);
            o.set_style_line_width(3, Selector::DEFAULT);
            o.set_style_line_color(hex(0xc6c6c6), Selector::DEFAULT);
        }

        // 唱片：位图 + 透明色 + 支点。支点必须在转起来之前定在盘心，否则会绕左上角转。
        {
            let src = ImageSrc::image(draw_disc());
            let mut o = disc.get_mut(lv)?;
            o.set_src(src);
            o.set_pos(DISC_X, DISC_Y);
            o.set_pivot((DISC / 2) as i32, (DISC / 2) as i32);
            o.set_style_image_colorkey(Some(&KEY), Selector::DEFAULT);
        }

        // 进度条：本首已播千分比。
        {
            let mut o = bar.get_mut(lv)?;
            o.set_pos(BAR_X, BAR_Y);
            o.set_size(BAR_W, BAR_H);
            o.view().set_range(0, 1000);
            o.set_style_bg_color(hex(0x48525c), Selector::DEFAULT);
            o.set_style_bg_color(hex(0xfc9800), Part::INDICATOR);
            o.set_style_radius(0, Selector::DEFAULT);
        }

        // 上一首 / 暂停 / 下一首：屏没有触摸，纯装饰。
        let symbols = [
            sys::LV_SYMBOL_PREV,
            sys::LV_SYMBOL_PAUSE,
            sys::LV_SYMBOL_NEXT,
        ];
        for (i, (btn, symbol)) in buttons.iter().zip(symbols).enumerate() {
            let label = {
                let mut o = btn.get_mut(lv)?;
                o.set_pos(BTN_X0 + i as i32 * (BTN_W + BTN_GAP), BTN_Y);
                o.set_size(BTN_W, BTN_H);
                o.set_style_bg_color(hex(0x262c34), Selector::DEFAULT);
                o.set_style_border_width(0, Selector::DEFAULT);
                o.set_style_radius(4, Selector::DEFAULT);

                let label = Label::create(&mut o)?;
                label.set_text_static(Some(symbol_text(symbol)))?;
                label.handle()?
            };
            let mut o = label.get_mut(lv)?;
            o.set_align(Align::CENTER);
            o.set_style_text_color(hex(0xffffff), Selector::DEFAULT);
        }

        // 歌词：固定成单行高度，位置与颜色由 [`Music::tick_lyrics`] 每帧改。
        for (label, line) in lyrics.iter().zip(LYRICS) {
            let mut o = label.get_mut(lv)?;
            o.set_pos(LYRIC_X, LYRIC_MID);
            o.set_size(LYRIC_W, LYRIC_STEP - 4);
            o.view().set_long_mode(LabelLongMode::DOTS);
            o.view().set_text_static(Some(line))?;
            o.set_style_text_color(hex(0x5c6b7a), Selector::DEFAULT);
        }

        Ok(Self {
            disc,
            arm,
            needle,
            bar,
            lyrics,
            scroll: 0,
            _owned: owned,
            screen,
        })
    }

    /// 把 t_ms 时刻的状态推给 LVGL：转盘角度、唱针位置、进度、歌词行。
    pub fn tick(&mut self, lv: &mut Lvgl, t_ms: u32) -> Result<(), Error> {
        let (song, pos) = locate(t_ms);
        let progress = (pos * 1000 / SONGS[song].ms) as i32;

        // 唱片整块旋转：LVGL 的角度单位是 0.1°，自己取模避免溢出
        let angle = ((t_ms as i32) * DEG_PER_S / 1000 % 3600) as i32;
        self.disc.get_mut(lv)?.set_rotation(angle);

        let (nx, ny) = update_arm(progress);
        set_arm_points(self.arm.get(lv)?);
        self.needle
            .get_mut(lv)?
            .set_pos(nx - NEEDLE_W / 2, ny - NEEDLE_H / 2);

        self.bar.get(lv)?.set_value(progress, false);

        self.tick_lyrics(lv, song, pos)
    }

    /// 歌词滚动：目标行号一阶低通，滚过去而不是跳行。
    fn tick_lyrics(&mut self, lv: &mut Lvgl, song: usize, pos: u32) -> Result<(), Error> {
        let count = SONGS[song].lines;
        let line = (pos as usize * count / SONGS[song].ms as usize).min(count - 1);
        let target = ((SONGS[song].first + line) as i32) << 8;

        self.scroll += (target - self.scroll) / 3;
        if (target - self.scroll).abs() < 16 {
            self.scroll = target;
        }

        for (i, label) in self.lyrics.iter().enumerate() {
            let delta = (i as i32 * 256 - self.scroll).div_euclid(256);
            let mut o = label.get_mut(lv)?;
            if delta.abs() > LYRIC_SPAN {
                o.set_hidden(true);
                continue;
            }
            o.set_hidden(false);
            o.set_pos(LYRIC_X, LYRIC_MID + delta * LYRIC_STEP);
            let color = if delta == 0 {
                hex(0xfc9800)
            } else {
                hex(0x5c6b7a)
            };
            o.set_style_text_color(color, Selector::DEFAULT);
        }
        Ok(())
    }
}

/// 每首歌曲的进度换算成唱针坐标：写进 [`ARM_PTS`]，并把针头中心返回给调用方。
///
/// 行程从标签外沿（内环）到盘边：进度 0 时针头正好在纹路最内侧。
fn update_arm(progress: i32) -> (i32, i32) {
    let (cx, cy) = (DISC_X + DISC_R, DISC_Y + DISC_R);
    let (nx, ny) = (
        cx + NEEDLE_NEAR.0 + (NEEDLE_FAR.0 - NEEDLE_NEAR.0) * progress / 1000,
        cy + NEEDLE_NEAR.1 + (NEEDLE_FAR.1 - NEEDLE_NEAR.1) * progress / 1000,
    );
    // SAFETY: 只有本任务在写这两个点，且写完立刻交给 `lv_line`。
    unsafe {
        let pts = &mut *addr_of_mut!(ARM_PTS);
        pts[0] = sys::lv_point_precise_t {
            x: PIVOT.0,
            y: PIVOT.1,
        };
        pts[1] = sys::lv_point_precise_t { x: nx, y: ny };
    }
    (nx, ny)
}

/// `lv_line_set_points` 的包装：`lvgl-rust` 没包它（LVGL 保留指针，安全层管不了生命周期）。
fn set_arm_points(arm: &Line) {
    // SAFETY: `ARM_PTS` 是 `static`，活得比线段长；两个点的内容由本任务独占写。
    unsafe {
        sys::lv_line_set_points(arm.as_ptr(), addr_of!(ARM_PTS).cast(), 2);
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

/// 按钮上的 LVGL 内置符号转成 C 字符串。
fn symbol_text(symbol: &[u8]) -> &CStr {
    CStr::from_bytes_with_nul(symbol).expect("lvgl symbol without nul")
}

/// 一次性画出唱片位图并返回它的描述符：盘面 + 同心纹路 + 四象限标签，旋转交给 LVGL。
fn draw_disc() -> &'static ImageDsc {
    // SAFETY: 位图与描述符都只由本函数初始化一次，之后只读；两者都是 `static`。
    unsafe {
        let px = &mut *addr_of_mut!(DISC_PX);
        let center = DISC as i32 / 2;

        for y in 0..DISC as i32 {
            for x in 0..DISC as i32 {
                let (dx, dy) = (x - center, y - center);
                let d2 = dx * dx + dy * dy;
                let color = if d2 > DISC_R * DISC_R || d2 <= 9 {
                    PX_KEY
                } else if d2 <= LABEL_R * LABEL_R {
                    if (dx > 0) == (dy > 0) {
                        PX_LABEL_A
                    } else {
                        PX_LABEL_B
                    }
                } else if d2 > (DISC_R - 2) * (DISC_R - 2) {
                    PX_RIM
                } else {
                    // 纹路用半径平方分档：比开方省事，越靠外越密正好是唱片的观感
                    let band = d2 / (LABEL_R * LABEL_R * GROOVE_STEP / 6);
                    if band % 2 == 0 { PX_GROOVE } else { PX_DISC }
                };

                let at = (y as usize * DISC + x as usize) * 2;
                let bytes = color.to_le_bytes();
                px.0[at] = bytes[0];
                px.0[at + 1] = bytes[1];
            }
        }

        let dsc = addr_of_mut!(DISC_DSC).cast::<ImageDsc>();
        dsc.write(ImageDsc::new(
            PixelFormat::Rgb565,
            DISC as u16,
            DISC as u16,
            DISC as u16 * 2,
            &*addr_of!(DISC_PX),
        ));
        &*dsc
    }
}
