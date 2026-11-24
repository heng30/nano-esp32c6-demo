//! 宿主机模拟器：把 1 bit 画布显示到 egui 窗口，并把窗口里的按键注入应用。
//!
//! 只有 `sim` 特性下才编译（本 crate 主体是 `#![no_std]`，这里为了窗口用 std）。
//! 应用只需要给一个闭包：每帧调用一次，在里面推进自己的 UI 逻辑并用
//! [`Context::show`] 交出要显示的画布。
//!
//! ```ignore
//! kk_ui::sim::run(
//!     kk_ui::sim::Options { title: "demo", width: 160, height: 120, scale: 2 },
//!     move |ctx| {
//!         ui.update(ctx.now, Input { keys: ctx.keys, encoder_delta: 0 });
//!         ctx.show(ui.frame());
//!     },
//! )?;
//! ```
//!
//! 底部按键栏对应 `KEY_UP` / `KEY_DOWN` / `KEY_OK` 与截图，键盘同效：
//! ↑/←/k/h 上、↓/→/j/l 下、回车确定、S 截图（存 `sim-<n>.png`）。

use std::borrow::ToOwned;
use std::boxed::Box;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::vec::Vec;

use eframe::egui;

use crate::{Frame, KEY_DOWN, KEY_OK, KEY_UP};

/// 底部按钮的字号（像素），是 egui 默认 14 的两倍。
const BUTTON_TEXT_SIZE: f32 = 28.0;
/// 底部按钮的横向内边距（像素）。
const BUTTON_PADDING_X: f32 = 18.0;
/// 底部按钮的纵向内边距（像素）。
const BUTTON_PADDING_Y: f32 = 8.0;
/// 一次按键的保持时长（毫秒），必须大于 KK_UI 的 20 ms 消抖。
const KEY_HOLD_MS: u32 = 60;
/// 窗口底部留给按键栏的高度（像素）。
const BAR_HEIGHT: f32 = 56.0;
/// 画布四周的留白（像素）。
const WINDOW_PADDING: f32 = 16.0;
/// 每帧的刷新间隔（毫秒）。
const REPAINT_MS: u64 = 5;
/// 窗口文字用的中文字体。
const CJK_FONT: &[u8] = include_bytes!("../assets/fonts/NotoSansCJKSC-Regular.otf");

/// 模拟器配置。
#[derive(Clone, Copy)]
pub struct Options {
    /// 窗口标题。
    pub title: &'static str,
    /// 逻辑画布宽度（像素）。
    pub width: i16,
    /// 逻辑画布高度（像素）。
    pub height: i16,
    /// 画布在窗口里的放大倍率。
    pub scale: usize,
}

/// 每帧交给应用的上下文。
pub struct Context<'a> {
    /// 仿真时间（毫秒），单调递增。
    pub now: u32,
    /// 本帧按住的按键位掩码，`KEY_UP` / `KEY_DOWN` / `KEY_OK` 的组合。
    pub keys: u8,
    /// 本帧要显示到窗口的画布，由 [`Context::show`] 填写。
    canvas: &'a mut Canvas,
}

impl Context<'_> {
    /// 提交本帧要显示的画布；像素没变时模拟器不会重传纹理。
    pub fn show(&mut self, frame: &Frame<'_>) {
        self.canvas.update(frame);
    }
}

/// 窗口要显示的一份画布。
struct Canvas {
    /// 1 bit 像素，布局同 [`Frame::buffer`]。
    pixels: Vec<u8>,
    /// 画布宽度（像素）。
    width: i16,
    /// 画布高度（像素）。
    height: i16,
    /// 自上次生成纹理后是否变过。
    dirty: bool,
}

impl Canvas {
    /// 空画布。
    fn new(width: i16, height: i16) -> Self {
        Self {
            pixels: Vec::new(),
            width,
            height,
            dirty: true,
        }
    }

    /// 换上新的画布内容；与当前内容一致时什么都不做。
    fn update(&mut self, frame: &Frame<'_>) {
        let pixels = frame.buffer();
        if self.width == frame.width()
            && self.height == frame.height()
            && self.pixels.as_slice() == pixels
        {
            return;
        }
        self.width = frame.width();
        self.height = frame.height();
        self.pixels.clear();
        self.pixels.extend_from_slice(pixels);
        self.dirty = true;
    }
}

/// 跑一个模拟器窗口，直到用户关掉；返回后可以再跑一次。
pub fn run<F>(options: Options, draw: F) -> Result<(), Box<dyn Error>>
where
    F: FnMut(&mut Context<'_>) + 'static,
{
    let scale = options.scale.max(1) as f32;
    let window = egui::vec2(
        options.width.max(0) as f32 * scale + WINDOW_PADDING,
        options.height.max(0) as f32 * scale + BAR_HEIGHT,
    );
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(window)
            .with_resizable(false),
        ..Default::default()
    };
    let sim = Sim {
        options,
        draw,
        canvas: Canvas::new(options.width, options.height),
        started: Instant::now(),
        keys: 0,
        keys_until: 0,
        texture: None,
        snapshots: 0,
        bar_width: 0.0,
    };
    eframe::run_native(
        options.title,
        native_options,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(sim))
        }),
    )?;
    Ok(())
}

/// 把画布按整数倍放大写成 8 位灰度 PNG（亮像素 255、灭像素 0）。
///
/// 模拟器自己按 S 截图时用它，无头调试也可以直接调。
pub fn save_png(frame: &Frame<'_>, scale: usize, path: &Path) -> Result<(), Box<dyn Error>> {
    let width = frame.width().max(0) as usize * scale;
    let height = frame.height().max(0) as usize * scale;
    let mut data = Vec::with_capacity(width * height);
    let mut row = std::vec![0u8; width];
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

/// 给 egui 装中文字体，否则按键栏上的中文是方块。
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "cjk".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(CJK_FONT)),
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

/// 一个模拟器窗口的状态。
struct Sim<F> {
    /// 画布尺寸与放大倍率。
    options: Options,
    /// 应用每帧的回调。
    draw: F,
    /// 应用交上来的画布。
    canvas: Canvas,
    /// 走时基准。
    started: Instant,
    /// 当前注入的按键位。
    keys: u8,
    /// 按键保持到的时间戳（毫秒）。
    keys_until: u32,
    /// 画布纹理。
    texture: Option<egui::TextureHandle>,
    /// 已保存的截图序号。
    snapshots: u32,
    /// 上一帧量到的按键栏宽度，用来把整行居中。
    bar_width: f32,
}

impl<F> Sim<F>
where
    F: FnMut(&mut Context<'_>),
{
    /// 注入一次按键，保持 [`KEY_HOLD_MS`]。
    fn press(&mut self, now: u32, keys: u8) {
        self.keys = keys;
        self.keys_until = now.wrapping_add(KEY_HOLD_MS);
    }

    /// 本帧真正按住的键。
    fn held(&self, now: u32) -> u8 {
        if self.keys != 0 && (now.wrapping_sub(self.keys_until) as i32) < 0 {
            self.keys
        } else {
            0
        }
    }

    /// 存一张截图到工作目录。
    fn save_snapshot(&mut self) {
        self.snapshots += 1;
        let path = PathBuf::from(std::format!("sim-{}.png", self.snapshots));
        let mut pixels = self.canvas.pixels.clone();
        let frame = Frame::new(&mut pixels, self.canvas.width, self.canvas.height);
        match save_png(&frame, self.options.scale, &path) {
            Ok(()) => std::println!("snapshot written: {}", path.display()),
            Err(error) => std::eprintln!("snapshot failed: {error}"),
        }
    }
}

impl<F> eframe::App for Sim<F>
where
    F: FnMut(&mut Context<'_>),
{
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = self.started.elapsed().as_millis() as u32;

        // 键盘与底部按键栏等价，外加 S 截图。
        let pressed = |key| ctx.input(|i| i.key_pressed(key));
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
                self.press(now, mapped);
            }
        }
        if pressed(egui::Key::S) {
            self.save_snapshot();
        }

        egui::Panel::bottom("kk-ui-keys").show(ui, |ui| {
            ui.style_mut().text_styles.insert(
                egui::TextStyle::Button,
                egui::FontId::proportional(BUTTON_TEXT_SIZE),
            );
            ui.style_mut().spacing.button_padding =
                egui::vec2(BUTTON_PADDING_X, BUTTON_PADDING_Y);

            // 按钮宽度由 egui 自己算，这里只把整行挪到中间：用上一帧量到的行宽，
            // 一帧就收敛（按钮组宽度不变，之后每帧都是准的）。
            let lead = ((ui.available_width() - self.bar_width) * 0.5).max(0.0);
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

        // 应用画一帧，画布交回来。
        let keys = self.held(now);
        {
            let Sim { draw, canvas, .. } = self;
            let mut context = Context { now, keys, canvas };
            draw(&mut context);
        }

        if self.texture.is_none() || self.canvas.dirty {
            let image = canvas_image(&self.canvas);
            match &mut self.texture {
                Some(texture) => texture.set(image, egui::TextureOptions::NEAREST),
                none => {
                    *none = Some(ctx.load_texture("kk-ui-canvas", image, egui::TextureOptions::NEAREST))
                }
            }
            self.canvas.dirty = false;
        }

        egui::CentralPanel::default().show(ui, |ui| {
            let Some(texture) = &self.texture else {
                return;
            };
            let size = egui::vec2(
                self.canvas.width.max(0) as f32 * self.options.scale as f32,
                self.canvas.height.max(0) as f32 * self.options.scale as f32,
            );
            let rect = egui::Rect::from_center_size(ui.available_rect_before_wrap().center(), size);
            ui.put(rect, egui::Image::new(texture).fit_to_exact_size(size));
        });

        // 应用一般每帧都推进动画，跟着它的节奏刷新即可。
        ctx.request_repaint_after(Duration::from_millis(REPAINT_MS));
    }
}

/// 1 bit 画布转成 egui 纹理：亮像素接近白，灭像素留一点底色。
fn canvas_image(canvas: &Canvas) -> egui::ColorImage {
    let (width, height) = (canvas.width.max(0) as usize, canvas.height.max(0) as usize);
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let index = x + (y >> 3) * width;
            let on = canvas
                .pixels
                .get(index)
                .is_some_and(|byte| byte & (1u8 << (y & 7)) != 0);
            let value = if on { 0xF0 } else { 0x18 };
            rgba.extend_from_slice(&[value, value, value, 0xFF]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba)
}
