//! 效果调度：第 0 页是 Slint 原生控件页，其余页由 Rust 逐像素渲染成一张位图。
//!
//! 一个效果一个文件，各自实现 [`Canvas`]，只负责往 RGB565 缓冲里写像素。

pub mod bars;
pub mod clock;
pub mod geometry;
pub mod music;
pub mod particles;
pub mod plasma;
pub mod tunnel;

use alloc::boxed::Box;
use embassy_time::{Duration, Instant};
use slint::platform::software_renderer::Rgb565Pixel;
use slint::{Image, SharedPixelBuffer};

use defmt::info;

use crate::MainWindow;

/// 画布宽（像素）。只有屏的一半：全屏 320×240 的 RGB565 位图要 150 KB，逐帧分配会把堆撑爆。
pub const WIDTH: usize = 160;
/// 画布高（像素），`.slint` 侧用 `image-fit: fill` 放大回 320×240。
pub const HEIGHT: usize = 120;

/// 一个像素效果：把 `t_ms` 时刻的这一帧画进 RGB565 缓冲。
pub trait Canvas {
    fn draw(&mut self, px: &mut [Rgb565Pixel], t_ms: u32);

    /// 页面还要在 Slint 侧显示文字时，把需要的状态推给界面；默认什么都不做。
    fn sync(&mut self, _ui: &MainWindow, _t_ms: u32) {}
}

/// 像素效果的数量，不含第 0 页的 Slint 控件页。
const PIXEL_PAGES: usize = 7;
/// 音乐播放器是最后一页，除了位图还要 Slint 画歌词。
const MUSIC_PAGE: usize = PIXEL_PAGES;
/// 每页停留时长。
const PAGE_DURATION: Duration = Duration::from_secs(8);
/// 音乐播放器页停留时长，够走完一遍播放列表。
const MUSIC_PAGE_DURATION: Duration = Duration::from_secs(40);
/// 控件页两个圆左右切换的间隔（毫秒）。
const DEMO_TOGGLE_MS: u64 = 2000;

/// 效果轮播器。
pub struct Effects {
    /// 界面句柄，Slint 组件本身就是 Rc，克隆很便宜。
    ui: MainWindow,
    /// 当前页号，0 是控件页。
    page: usize,
    /// 当前页的起始时刻。
    switched_at: Instant,
    /// 刚换过页：新画布要先清一次底，免得看到上一页的残留。
    fresh: bool,
    /// 各像素效果，下标 = 页号 - 1。
    canvases: [Box<dyn Canvas>; PIXEL_PAGES],
}

impl Effects {
    pub fn new(ui: MainWindow) -> Self {
        Self {
            ui,
            page: 0,
            switched_at: Instant::now(),
            fresh: true,
            canvases: [
                Box::new(geometry::Geometry),
                Box::new(particles::Particles::new()),
                Box::new(tunnel::Tunnel),
                Box::new(bars::Bars::new()),
                Box::new(clock::Clock),
                Box::new(plasma::Plasma),
                Box::new(music::Music::new()),
            ],
        }
    }

    /// 推进一帧：到点换页，然后只更新当前页。
    pub fn tick(&mut self) {
        let now = Instant::now();
        let duration = if self.page == MUSIC_PAGE {
            MUSIC_PAGE_DURATION
        } else {
            PAGE_DURATION
        };
        if now - self.switched_at >= duration {
            self.page = (self.page + 1) % (PIXEL_PAGES + 1);
            self.switched_at = now;
            self.fresh = true;
            self.ui.set_active(self.page as i32);
            info!("effect page {}", self.page);
        }

        let elapsed_ms = (now - self.switched_at).as_millis();
        if self.page == 0 {
            let on = (elapsed_ms / DEMO_TOGGLE_MS).is_multiple_of(2);
            self.ui.set_demo_counter(if on { 1 } else { 0 });
            return;
        }

        let mut buffer = SharedPixelBuffer::<Rgb565Pixel>::new(WIDTH as u32, HEIGHT as u32);
        if self.fresh {
            buffer.make_mut_slice().fill(Rgb565Pixel(0));
            self.fresh = false;
        }
        self.canvases[self.page - 1].draw(buffer.make_mut_slice(), elapsed_ms as u32);
        self.ui.set_frame(Image::from_rgb565(buffer));
        self.canvases[self.page - 1].sync(&self.ui, elapsed_ms as u32);
    }
}

/// 角度（度）转 sin 定点值 Q10（1024 = 1.0），Bhaskara 近似，误差 < 2%。
pub(crate) fn sin_q10(deg: i32) -> i32 {
    let deg = deg.rem_euclid(360);
    let (x, sign) = if deg < 180 { (deg, 1) } else { (deg - 180, -1) };
    let t = x * (180 - x);
    sign * (4 * t * 1024 / (40500 - t))
}
