//! 本地仿真：把 1 bit 画布放大到 eframe 窗口，底部三个按钮模拟上/下/确定。
//!
//! 窗口模式（默认）：`cargo run --example sim --features sim`
//! 无头截图：`cargo run --example sim --features sim -- --snapshot out.png --steps 40`
//! 底部按钮与键盘等价：↑/←/k/h 上、↓/→/j/l 下、回车确定、S 存一张 `sim-<n>.png` 截图。
//! 窗口固定大小、内容居中，egui 侧文字用 `assets/fonts/NotoSansCJKSC-Regular.otf`。
//!
//! 应用描述表是 `app.rs`（KK_UI 标准 Demo，复制自 `ili9341-lcd-kkui`），
//! 字体与图标在 `assets/`，由它自己按相对路径嵌入。

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use kk_ui::{Frame, Input, KEY_DOWN, KEY_OK, KEY_UP, KkUi};

mod app;

/// 逻辑画布宽度（像素）。
const CANVAS_WIDTH: i16 = 160;
/// 逻辑画布高度（像素）。
const CANVAS_HEIGHT: i16 = 120;
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
/// 底部按钮的字号（像素），是 egui 默认 14 的两倍。
const BUTTON_TEXT_SIZE: f32 = 28.0;

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

    match snapshot {
        Some(path) => {
            let mut sim = Sim::new();
            sim.run_autoplay(steps);
            save_png(sim.ui.frame(), SCALE, &path)?;
            println!("snapshot written: {}", path.display());
            Ok(())
        }
        None => run_window(),
    }
}

fn run_window() -> Result<(), Box<dyn Error>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([
                CANVAS_WIDTH as f32 * SCALE as f32 + 16.0,
                CANVAS_HEIGHT as f32 * SCALE as f32 + 56.0,
            ])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "kk-ui sim",
        options,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(Sim::new()))
        }),
    )?;
    Ok(())
}

/// 给 egui 装中文字体，否则按钮上的中文是方块。
fn install_fonts(ctx: &egui::Context) {
    const CJK: &[u8] = include_bytes!("assets/fonts/NotoSansCJKSC-Regular.otf");
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "cjk".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(CJK)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "cjk".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// 把画布按整数倍放大写成 8 位灰度 PNG：亮像素 255、灭像素 0，
/// 横向用 `Frame::expand_row` 展开，纵向按 `scale` 重复。
fn save_png(frame: &Frame<'_>, scale: usize, path: &Path) -> Result<(), Box<dyn Error>> {
    let width = frame.width().max(0) as usize * scale;
    let height = frame.height().max(0) as usize * scale;
    let mut data = Vec::with_capacity(width * height);
    let mut row = vec![0u8; width];
    for y in 0..frame.height().max(0) {
        let written = frame.expand_row(y, scale, 255u8, 0u8, &mut row);
        for _ in 0..scale {
            data.extend_from_slice(&row[..written]);
        }
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width as u32, height as u32);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&data)?;
    Ok(())
}

/// 一次仿真会话：UI 运行时 + 按键注入 + 窗口纹理。
struct Sim {
    /// UI 运行时，帧缓冲泄漏成 `'static` 后由它独占。
    ui: KkUi<'static, app::Wave>,
    /// 走时基准，毫秒。
    started: Instant,
    /// 当前按住的按键位。
    keys: u8,
    /// 按键保持到的时间戳（毫秒）。
    keys_until: u32,
    /// 画布纹理，窗口模式才有。
    texture: Option<egui::TextureHandle>,
    /// 已保存的截图序号。
    snapshots: u32,
    /// 上一帧量到的底部按钮行宽度，用来把整行居中。
    bar_width: f32,
}

impl Sim {
    fn new() -> Self {
        let buffer: &'static mut [u8] =
            Box::leak(vec![0u8; Frame::buffer_len(CANVAS_WIDTH, CANVAS_HEIGHT)].into_boxed_slice());
        let mut ui = KkUi::new(
            &app::APP,
            app::Wave::new(),
            buffer,
            CANVAS_WIDTH,
            CANVAS_HEIGHT,
        );
        ui.init();
        app::reset(&mut ui, 0);
        Self {
            ui,
            started: Instant::now(),
            keys: 0,
            keys_until: 0,
            texture: None,
            snapshots: 0,
            bar_width: 0.0,
        }
    }

    /// 注入一次按键，保持 [`KEY_HOLD_MS`]。
    fn press(&mut self, now: u32, keys: u8) {
        self.keys = keys;
        self.keys_until = now.wrapping_add(KEY_HOLD_MS);
    }

    /// 推进一拍：按键、UI 更新与应用业务。
    fn tick(&mut self, now: u32) {
        let keys = if self.keys != 0 && (now.wrapping_sub(self.keys_until) as i32) < 0 {
            self.keys
        } else {
            0
        };
        self.ui.update(
            now,
            Input {
                keys,
                encoder_delta: 0,
            },
        );
        app::update(&mut self.ui, now);
        app::process_events(&mut self.ui);
    }

    /// 无头跑一段自动演示，最后停在某一页。
    fn run_autoplay(&mut self, steps: u32) {
        let mut autoplay = app::Autoplay::new();
        let mut now = 0;
        for _ in 0..steps {
            let key = autoplay.next_key(&self.ui);
            self.press(now, key);
            for _ in 0..AUTOPLAY_INTERVAL_MS / TICK_MS {
                self.tick(now);
                now += TICK_MS;
            }
        }
    }

    /// 存一张截图到工作目录。
    fn save_snapshot(&mut self) {
        self.snapshots += 1;
        let path = PathBuf::from(format!("sim-{}.png", self.snapshots));
        match save_png(self.ui.frame(), SCALE, &path) {
            Ok(()) => println!("snapshot written: {}", path.display()),
            Err(error) => eprintln!("snapshot failed: {error}"),
        }
    }
}

impl eframe::App for Sim {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = self.started.elapsed().as_millis() as u32;

        // 键盘与底部按钮等价：↑/←/k/h 上、↓/→/j/l 下、回车确定、S 截图。
        let pressed = |key| ctx.input(|i| i.key_pressed(key));
        let mut keys = 0;
        for (key, mapped) in [
            (egui::Key::ArrowUp, KEY_UP),
            (egui::Key::ArrowLeft, KEY_UP),
            (egui::Key::K, KEY_UP),
            (egui::Key::H, KEY_UP),
            (egui::Key::ArrowDown, KEY_DOWN),
            (egui::Key::ArrowRight, KEY_DOWN),
            (egui::Key::J, KEY_DOWN),
            (egui::Key::L, KEY_DOWN),
            (egui::Key::Enter, KEY_OK),
        ] {
            if pressed(key) {
                keys |= mapped;
            }
        }
        if keys != 0 {
            self.press(now, keys);
        }
        if pressed(egui::Key::S) {
            self.save_snapshot();
        }
        self.tick(now);

        if self.texture.is_none() || self.ui.take_frame_ready() {
            let image = canvas_image(self.ui.frame());
            match &mut self.texture {
                Some(texture) => texture.set(image, egui::TextureOptions::NEAREST),
                none => {
                    *none = Some(ctx.load_texture("canvas", image, egui::TextureOptions::NEAREST))
                }
            }
        }

        egui::Panel::bottom("keys").show(ui, |ui| {
            let font = egui::FontId::proportional(BUTTON_TEXT_SIZE);
            let padding = egui::vec2(18.0, 8.0);
            ui.style_mut()
                .text_styles
                .insert(egui::TextStyle::Button, font);
            ui.style_mut().spacing.button_padding = padding;

            // 按钮宽度由 egui 自己算，这里只把整行挪到中间：用上一帧量到的行宽，
            // 一帧就收敛（按钮组宽度不变，之后每帧都是准的）。
            let avail = ui.available_width();
            let lead = ((avail - self.bar_width) * 0.5).max(0.0);
            let row = ui.horizontal(|ui| {
                ui.add_space(lead);
                for (label, key) in [("上", KEY_UP), ("下", KEY_DOWN), ("确定", KEY_OK)] {
                    if ui.button(label).clicked() {
                        self.press(now, key);
                    }
                }
                if ui.button("截图").clicked() {
                    self.save_snapshot();
                }
            });
            self.bar_width = row.response.rect.width() - lead;
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let size = egui::vec2(
                CANVAS_WIDTH as f32 * SCALE as f32,
                CANVAS_HEIGHT as f32 * SCALE as f32,
            );
            let rect = egui::Rect::from_center_size(ui.available_rect_before_wrap().center(), size);
            if let Some(texture) = &self.texture {
                ui.put(rect, egui::Image::new(texture).fit_to_exact_size(size));
            }
        });

        // 库自身 20 ms 出一帧，窗口跟着走即可。
        ctx.request_repaint_after(Duration::from_millis(TICK_MS as u64));
    }
}

/// 1 bit 画布转成 egui 纹理：亮像素接近白，灭像素留一点底色。
fn canvas_image(frame: &Frame<'_>) -> egui::ColorImage {
    let (width, height) = (frame.width().max(0) as usize, frame.height().max(0) as usize);
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height as i16 {
        for x in 0..width as i16 {
            let value = if frame.pixel(x, y) { 0xF0 } else { 0x18 };
            rgba.extend_from_slice(&[value, value, value, 0xFF]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba)
}
