//! 用 `kk_ui::sim` 在主机上跑 KK_UI 标准 Demo。
//!
//! 字库由 `Makefile` 先按本目录用到的字符生成（编译期 `include_bytes!`，缺文件直接编译失败）：
//!
//! ```text
//! make            # 生成字库 + 开窗口跑
//! make font       # 只生成字库
//! make snapshot   # 无头截图，SNAPSHOT=/tmp/sim.png STEPS=40
//! ```
//!
//! 界面逻辑全在 `app.rs`（KK_UI 标准 Demo 的副本，图标在 `assets/`）；这里只把字库
//! 和 `KkUi` 接进模拟器闭包：每帧推进一次，再把画布交还给窗口。

mod app;

use std::error::Error;
use std::path::{Path, PathBuf};

use kk_ui::sim;
use kk_ui::{Frame, Glyphs, Input, KkUi, set_font};

/// `kkui-tool font` 生成的字模，每 32 字节一个字。
static BITMAPS: &[u8] = include_bytes!("assets/font/font.bin");
/// 与 `BITMAPS` 一一对应的像素宽度。
static WIDTHS: &[u8] = include_bytes!("assets/font/width.bin");
/// 与 `BITMAPS` 一一对应的码点，u16 小端升序。
static CODEPOINTS: &[u8] = include_bytes!("assets/font/cp.bin");
/// 本示例专用的字库：只含 `examples/sim` 里真正用到的字符。
static GLYPHS: Glyphs = Glyphs {
    bitmaps: BITMAPS,
    widths: WIDTHS,
    codepoints: CODEPOINTS,
};

/// 逻辑画布宽度（像素）。
const WIDTH: i16 = 160;
/// 逻辑画布高度（像素）。
const HEIGHT: i16 = 120;
/// 窗口与截图里的放大倍率。
const SCALE: usize = 2;
/// 一次按键的保持时长（毫秒），必须大于 KK_UI 的 20 ms 消抖。
const KEY_HOLD_MS: u32 = 60;
/// 无头模式的仿真节拍（毫秒）。
const TICK_MS: u32 = 5;
/// 无头模式的自动演示间隔（毫秒），与板上 Demo 一致。
const AUTOPLAY_INTERVAL_MS: u32 = 500;
/// 无头模式默认推进的拍数。
const DEFAULT_STEPS: u32 = 40;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let mut snapshot = None;
    let mut steps = DEFAULT_STEPS;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--snapshot" => {
                let path = args.next().ok_or("--snapshot needs a path")?;
                snapshot = Some(PathBuf::from(path));
            }
            "--steps" => steps = args.next().ok_or("--steps needs a count")?.parse()?,
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    set_font(&GLYPHS);
    let buffer: &'static mut [u8] =
        Box::leak(vec![0u8; Frame::buffer_len(WIDTH, HEIGHT)].into_boxed_slice());
    let mut ui = KkUi::new(&app::APP, app::Wave::new(), buffer, WIDTH, HEIGHT);
    ui.init();
    app::reset(&mut ui, 0);

    match snapshot {
        Some(path) => headless(&mut ui, steps, &path),
        None => sim::run(
            sim::Options {
                title: "kk-ui demo",
                width: WIDTH,
                height: HEIGHT,
                scale: SCALE,
            },
            move |ctx| {
                ui.update(
                    ctx.now,
                    Input {
                        keys: ctx.keys,
                        encoder_delta: 0,
                    },
                );
                app::update(&mut ui, ctx.now);
                app::process_events(&mut ui);
                ctx.show(ui.frame());
            },
        ),
    }
}

/// 无头模式：让 Demo 自己按 `Autoplay` 走 `steps` 拍，然后存一张 PNG。
fn headless(ui: &mut KkUi<'_, app::Wave>, steps: u32, path: &Path) -> Result<(), Box<dyn Error>> {
    let mut autoplay = app::Autoplay::new();
    let mut now = 0;
    // 先送一拍空输入，把去抖状态机的初始电平记为松开，首拍按键才不会被吞。
    ui.update(
        now,
        Input {
            keys: 0,
            encoder_delta: 0,
        },
    );
    now += TICK_MS;
    for _ in 0..steps {
        let keys = autoplay.next_key(ui);
        let until = now + KEY_HOLD_MS;
        for _ in 0..AUTOPLAY_INTERVAL_MS / TICK_MS {
            let held = if keys != 0 && now < until { keys } else { 0 };
            ui.update(
                now,
                Input {
                    keys: held,
                    encoder_delta: 0,
                },
            );
            app::update(ui, now);
            app::process_events(ui);
            now += TICK_MS;
        }
    }
    sim::save_png(ui.frame(), SCALE, path)
}
