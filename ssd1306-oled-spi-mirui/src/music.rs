//! 音乐播放器页：左边自绘转盘与唱臂，右边曲名、进度条与三行歌词。
//!
//! 左边是 `#[view]` 自绘控件（mirui 的软件光栅器直接吃 `DrawCommand`）：
//! 盘面只用白线画外沿、纹路环与中心标签，转动靠一条绕盘心扫过去的白线，
//! 唱针沿纹路向外爬。右边全是现成控件：`Text` 读 [`Signal`]，`ProgressBar` 读同一个信号。
//!
//! 屏只有 128×64 且是 1 bit，所以配色只用黑白两色、线宽不低于 1 px。

use mirui::prelude::draw::*;
use mirui::prelude::*;
use mirui::render::scene::Paint;
use mirui::types::Transform;
use mirui::ui::widgets::{ParagraphStyle, ProgressBar, Text, TextAlign};

/// 屏高（像素）。
pub const HEIGHT: i32 = 64;

/// 左侧转盘区宽度（像素）。
const DECK_W: i32 = 58;
/// 盘心（转盘区坐标）。
const DISC_C: (i32, i32) = (28, 36);
/// 唱片半径。
const DISC_R: i32 = 22;
/// 中心标签半径：纹路从这里开始。
const LABEL_R: i32 = 7;
/// 纹路环间隔（像素）。
const GROOVE_STEP: i32 = 5;
/// 唱臂支点（转盘区坐标），在盘外右上。
const PIVOT: (i32, i32) = (53, 5);
/// 唱针行程：进度 0 在标签外沿，进度 1000 在盘边内侧。
const NEEDLE_R0: i32 = LABEL_R + 2;
const NEEDLE_R1: i32 = DISC_R - 3;
/// 唱针所在方位的角度（度），右上。
const NEEDLE_DEG: i32 = 42;
/// 转速 33⅓ rpm ≈ 200°/s。
const DEG_PER_S: u32 = 200;

const WHITE: Color = Color::rgb(255, 255, 255);
const BLACK: Color = Color::rgb(0, 0, 0);

/// 一首歌：时长、曲名与歌词（歌词都很短，128 px 宽只放得下一行十来个字符）。
struct Song {
    /// 时长，毫秒。
    ms: u32,
    /// 曲名。
    title: &'static str,
    /// 歌词，按播放顺序。
    lines: &'static [&'static str],
}

/// 播放列表，走完 [`CYCLE_MS`] 后回到第一首。
const SONGS: [Song; 3] = [
    Song {
        ms: 12_000,
        title: "NEON",
        lines: &["neon over", "silicon", "ride empty", "bus"],
    },
    Song {
        ms: 9_000,
        title: "STATIC",
        lines: &["static on", "the radio", "sings the", "city"],
    },
    Song {
        ms: 15_000,
        title: "GROOVE",
        lines: &["dance on", "black", "needle in", "groove"],
    },
];

/// 播放列表总时长（毫秒）。
const CYCLE_MS: u32 = 12_000 + 9_000 + 15_000;

/// 右边一列的当前读数，由 [`playback_tick`] 每帧写进 [`Signal`]。
#[derive(Clone, Copy, PartialEq)]
pub struct Readout {
    /// 曲名。
    title: &'static str,
    /// 上一行歌词（已唱过）。
    prev: &'static str,
    /// 当前行歌词，反白显示。
    now: &'static str,
    /// 下一行歌词。
    next: &'static str,
    /// 本首已播比例，0–1。
    progress: Fixed,
}

impl Default for Readout {
    fn default() -> Self {
        Self {
            title: "",
            prev: "",
            now: "",
            next: "",
            progress: Fixed::ZERO,
        }
    }
}

/// 播放状态在 ECS 里的句柄：`ui!` 的绑定要读它，系统每帧写它。
pub struct Playback(pub Signal<Readout>);

/// 转盘控件的每帧脏标记：画什么全在 [`turntable_render`] 里自己算，这里只要它被重画。
#[component]
#[derive(Clone, Copy, Default)]
pub struct Turntable {}

/// 把播放进度写进信号，并在动画阶段标脏转盘。
#[mirui::system]
pub fn playback_tick(
    playback: res!(Playback),
    clock: res!(optional MonoClock),
    deck: query!(mut Turntable),
) {
    let now_ms = clock.map_or(0, |clock| clock.now_ms());
    playback.0.set(readout(now_ms));
    for _deck in deck {}
}

/// 把 t 时刻换算成（正在放的那首，本首已播毫秒）。
fn locate(t_ms: u32) -> (&'static Song, u32) {
    let mut pos = t_ms % CYCLE_MS;
    for song in &SONGS {
        if pos < song.ms {
            return (song, pos);
        }
        pos -= song.ms;
    }
    (&SONGS[SONGS.len() - 1], 0)
}

/// t 时刻应该显示的读数。
fn readout(t_ms: u32) -> Readout {
    let (song, pos) = locate(t_ms);
    let count = song.lines.len();
    let line = (pos as usize * count / song.ms as usize).min(count - 1);
    let at = |offset: isize| -> &'static str {
        let index = line as isize + offset;
        if index < 0 || index as usize >= count {
            ""
        } else {
            song.lines[index as usize]
        }
    };

    Readout {
        title: song.title,
        prev: at(-1),
        now: at(0),
        next: at(1),
        progress: Fixed::from_ratio(pos as i32, song.ms as i32),
    }
}

/// 转盘当前状态：转动角度（度）与唱针行程（0–1000）。
fn deck_state(t_ms: u32) -> (u32, i32) {
    let (song, pos) = locate(t_ms);
    let spin = (t_ms % (3_600_000 / DEG_PER_S)) * DEG_PER_S / 1000;
    (spin, (pos * 1000 / song.ms) as i32)
}

#[compose]
pub fn build_page(state: Signal<Readout>) {
    // 每个绑定各拿走一份 Signal（内部是 Rc），同一个句柄不能同时被五个闭包捕获。
    let (title, bar, prev, now, next) = (
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state,
    );
    ui! {
        Row (grow: 1.0, bg_color: ColorToken::Surface) {
            Turntable (width: DECK_W, height: HEIGHT)
            Column (
                grow: 1.0,
                height: HEIGHT,
                padding: Padding {
                    left: Dimension::px(2),
                    right: Dimension::px(3),
                    top: Dimension::px(3),
                    bottom: Dimension::px(3),
                },
                row_gap: 2
            ) {
                Text (
                    text: ${ title.get().title },
                    width: Dimension::percent(100),
                    height: 10,
                    font_size: 8,
                    text_color: ColorToken::OnSurface,
                    paragraph: ParagraphStyle::label().with_align(TextAlign::Start)
                )
                ProgressBar (
                    width: Dimension::percent(100),
                    height: 4,
                    border_radius: 0,
                    value: ${ bar.get().progress.to_f32() }
                )
                Text (
                    text: ${ prev.get().prev },
                    width: Dimension::percent(100),
                    height: 10,
                    font_size: 6,
                    text_color: ColorToken::OnSurface,
                    paragraph: ParagraphStyle::label().with_align(TextAlign::Start)
                )
                Text (
                    text: ${ now.get().now },
                    width: Dimension::percent(100),
                    height: 10,
                    font_size: 6,
                    bg_color: ColorToken::OnSurface,
                    text_color: ColorToken::Surface,
                    paragraph: ParagraphStyle::label().with_align(TextAlign::Start)
                )
                Text (
                    text: ${ next.get().next },
                    width: Dimension::percent(100),
                    height: 10,
                    font_size: 6,
                    text_color: ColorToken::OnSurface,
                    paragraph: ParagraphStyle::label().with_align(TextAlign::Start)
                )
            }
        }
    };
}

pub use turntable_render::view as turntable_view;

/// 单位圆（半径 100），所有圆都按它缩放。
static CIRCLE: Path = mirui::path!(
    "M 100 0 C 100 55.228 55.228 100 0 100 C -55.228 100 -100 55.228 -100 0 C -100 -55.228 -55.228 -100 0 -100 C 55.228 -100 100 -55.228 100 0 Z"
);

#[mirui::view(component = Turntable, name = "Turntable", priority = 60)]
fn turntable_render(_state: component!(), clock: res!(optional MonoClock), paint: paint!()) {
    let rect = paint.rect();
    let (spin, progress) = deck_state(clock.map_or(0, |clock| clock.now_ms()));

    let cx = rect.x + fx(DISC_C.0);
    let cy = rect.y + fx(DISC_C.1);
    let centre = Point { x: cx, y: cy };
    let origin = paint.transform();

    // 盘面只画白线：外沿一圈 + 几圈纹路 + 中心标签，盘体本身留黑，
    // 这样白唱臂压在盘上也看得见（反过来的话白臂直接消失在白盘里）。
    ring(paint, origin, centre, DISC_R, WHITE);
    let mut r = LABEL_R + GROOVE_STEP;
    while r < DISC_R - 1 {
        ring(paint, origin, centre, r, WHITE);
        r += GROOVE_STEP;
    }
    disc(paint, origin, DISC_C, LABEL_R, WHITE);
    disc(paint, origin, DISC_C, 2, BLACK);

    // 转动：从标签外沿甩出去的一条白线，绕盘心转，压在纹路上也还是白的。
    let angle = fx(spin as i32);
    stroke(
        paint,
        origin,
        offset(toward(angle, LABEL_R), DISC_C),
        offset(toward(angle, DISC_R - 3), DISC_C),
        fx(2),
        WHITE,
    );

    // 唱臂：支点到唱针，针头比臂粗一格，看得出是「头」。
    let needle = offset(
        toward(
            fx(NEEDLE_DEG),
            NEEDLE_R0 + (NEEDLE_R1 - NEEDLE_R0) * progress / 1000,
        ),
        DISC_C,
    );
    stroke(paint, origin, point(PIVOT), needle, fx(1), WHITE);
    disc(paint, origin, PIVOT, 2, WHITE);
    block(
        paint,
        origin,
        Rect::new(needle.x - fx(1), needle.y - fx(1), 3, 3),
        0,
        WHITE,
    );
}

fn fx(v: i32) -> Fixed {
    Fixed::from_int(v)
}

/// (x, y) 像素坐标转点。
fn point((x, y): (i32, i32)) -> Point {
    Point { x: fx(x), y: fx(y) }
}

/// 相对 `(cx, cy)` 偏移 `(dx, dy)`。
fn offset(p: Point, (cx, cy): (i32, i32)) -> Point {
    Point {
        x: p.x + fx(cx),
        y: p.y + fx(cy),
    }
}

/// 自 12 点方向顺时针 `angle` 度、半径 `r` 处的点（原点在圆心）。
fn toward(angle: Fixed, r: i32) -> Point {
    Point {
        x: Fixed::sin_deg(angle) * fx(r),
        y: Fixed::ZERO - Fixed::cos_deg(angle) * fx(r),
    }
}

/// 填一个圆心 `(cx, cy)`、半径 `r` 的圆。
fn disc(paint: &mut PaintCtx<'_>, t: Transform, (cx, cy): (i32, i32), r: i32, color: Color) {
    let placed = t.compose(
        &Transform::translate(fx(cx), fx(cy)).compose(&Transform::scale(
            Fixed::from_ratio(r, 100),
            Fixed::from_ratio(r, 100),
        )),
    );
    paint.draw(&DrawCommand::FillPath {
        path: &CIRCLE,
        transform: placed,
        paint: &Paint::Color(color.into()),
        opa: 255,
        fill_rule: FillRule::NonZero,
    });
}

/// 画一圈 1 px 的环。
fn ring(paint: &mut PaintCtx<'_>, t: Transform, centre: Point, r: i32, color: Color) {
    paint.draw(&DrawCommand::Arc {
        center: centre,
        transform: t,
        radius: fx(r),
        start_angle: Fixed::ZERO,
        end_angle: fx(360),
        color,
        width: fx(1),
        opa: 255,
    });
}

/// 画一条实心线段。
fn stroke(paint: &mut PaintCtx<'_>, t: Transform, p1: Point, p2: Point, w: Fixed, c: Color) {
    paint.draw(&DrawCommand::Line {
        p1,
        p2,
        transform: t,
        color: c,
        width: w,
        opa: 255,
    });
}

/// 填一个矩形块。
fn block(paint: &mut PaintCtx<'_>, t: Transform, area: Rect, radius: i32, color: Color) {
    paint.draw(&DrawCommand::Fill {
        area,
        transform: t,
        quad: None,
        color,
        radius: fx(radius),
        opa: 255,
    });
}
