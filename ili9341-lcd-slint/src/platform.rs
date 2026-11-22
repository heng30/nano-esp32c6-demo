//! Slint 的 MCU 平台适配：窗口、时间源，以及逐行刷屏。
//!
//! 裸机上没有窗口系统，Slint 只要求实现 `Platform`；`MinimalSoftwareWindow` 负责软件渲染，
//! 渲染结果按行回调给我们，直接写进并口。

use alloc::{boxed::Box, rc::Rc};
use slint::platform::{
    Platform, WindowAdapter,
    software_renderer::{
        LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel,
    },
};

use crate::display::Display;

/// 屏宽（像素，横屏）。
pub const WIDTH: usize = 320;
/// 屏高（像素，横屏）。
pub const HEIGHT: usize = 240;

/// 注册平台并返回窗口句柄。
///
/// `set_platform` 只能成功调用一次，所以这里一次性把窗口建好、克隆出去。
pub fn init() -> Rc<MinimalSoftwareWindow> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    window.set_size(slint::PhysicalSize::new(WIDTH as u32, HEIGHT as u32));

    slint::platform::set_platform(Box::new(EspPlatform {
        window: window.clone(),
    }))
    .expect("slint platform already set");

    window
}

/// 只提供窗口适配器与时间源，事件循环留在 `main` 里自己跑。
struct EspPlatform {
    /// 单窗口设备，`create_window_adapter` 直接把它克隆出去。
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for EspPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> core::time::Duration {
        core::time::Duration::from_micros(embassy_time::Instant::now().as_micros())
    }
}

/// 一行像素的落屏器：Slint 渲染好一行就写一行。
///
/// 只占一行缓冲（640 B），比整帧缓冲省 150 KB；脏区刷新时单独一行的开销是一次窗口设置
/// 加 320 个像素，实测远快于先攒整帧再刷。
pub struct LineWriter<'a> {
    /// 并口屏。
    pub display: &'a mut Display,
    /// Slint 的 RGB565 行缓冲。
    pub pixels: &'a mut [Rgb565Pixel; WIDTH],
    /// 转成 `draw_raw_slice` 需要的裸 u16 行缓冲。
    pub raw: &'a mut [u16; WIDTH],
    /// 已刷新的行数，用来确认界面真的在画。
    pub rows: &'a mut usize,
}

impl LineBufferProvider for LineWriter<'_> {
    type TargetPixel = Rgb565Pixel;

    fn process_line(
        &mut self,
        line: usize,
        range: core::ops::Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        render_fn(&mut self.pixels[range.clone()]);
        *self.rows += 1;

        for (dst, px) in self.raw[range.clone()]
            .iter_mut()
            .zip(&self.pixels[range.clone()])
        {
            *dst = px.0;
        }

        self.display
            .draw_raw_slice(
                range.start as u16,
                line as u16,
                (range.end - 1) as u16,
                line as u16,
                &self.raw[range.clone()],
            )
            .expect("line flush failed");
    }
}
