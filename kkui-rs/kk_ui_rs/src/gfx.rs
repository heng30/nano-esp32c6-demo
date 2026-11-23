//! 128×64 单色画布：页式帧缓冲、半开裁剪窗口、绘图模式与图元。
//!
//! 布局与 KK_OLED 一致：每字节是同列连续 8 行，位序按行号从低到高。
//! 坐标一律是有符号 `i16`，裁剪窗口是半开区间 `[x0, x1) × [y0, y1)`。

use crate::font::Font;

/// 逻辑画布宽度（像素）。
pub const WIDTH: i16 = 128;
/// 逻辑画布高度（像素）。
pub const HEIGHT: i16 = 64;
/// 页式缓冲字节数。
const BUF_LEN: usize = WIDTH as usize * (HEIGHT as usize / 8);

/// 前景像素如何作用于帧缓冲。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DrawMode {
    /// 前景像素清零。
    Clear,
    /// 前景像素置一。
    Set,
    /// 前景像素与原值异或。
    Xor,
}

/// 单色画布与绘制状态。
pub struct Frame {
    buf: [u8; BUF_LEN],
    /// 半开裁剪窗口 `(x0, y0, x1, y1)`。
    clip: (i16, i16, i16, i16),
    mode: DrawMode,
    font: Option<&'static Font>,
    /// 当前字体的上升量（像素）。
    ascent: i16,
    /// 当前字体的下降量（像素，负值）。
    descent: i16,
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}

impl Frame {
    /// 空白画布：全部像素清零，裁剪窗口为整屏，绘图模式为置一。
    pub const fn new() -> Self {
        Self {
            buf: [0; BUF_LEN],
            clip: (0, 0, WIDTH, HEIGHT),
            mode: DrawMode::Set,
            font: None,
            ascent: 0,
            descent: 0,
        }
    }

    /// 原始帧缓冲，供上层放大显示。
    pub fn buffer(&self) -> &[u8; BUF_LEN] {
        &self.buf
    }

    /// 读取一个逻辑坐标像素。
    pub fn pixel(&self, x: i16, y: i16) -> bool {
        if !(0..WIDTH).contains(&x) || !(0..HEIGHT).contains(&y) {
            return false;
        }
        let index = x as usize + (y as usize >> 3) * WIDTH as usize;
        self.buf[index] & (1u8 << (y & 7)) != 0
    }

    /// 清空全部像素；裁剪窗口与绘图模式保持不变。
    pub fn clear(&mut self) {
        self.buf.fill(0);
    }

    /// 设置后续图元的绘图模式。
    pub fn set_draw_mode(&mut self, mode: DrawMode) {
        self.mode = mode;
    }

    /// 设置半开裁剪窗口；宽高超出 `i16` 表示范围时保持原窗口不变。
    pub fn set_clip(&mut self, x: i16, y: i16, width: u16, height: u16) {
        if width > i16::MAX as u16 || height > i16::MAX as u16 {
            return;
        }
        let mut x1 = x as i32 + width as i32;
        let mut y1 = y as i32 + height as i32;
        let x0 = x.clamp(0, WIDTH);
        let y0 = y.clamp(0, HEIGHT);
        x1 = x1.clamp(x0 as i32, WIDTH as i32);
        y1 = y1.clamp(y0 as i32, HEIGHT as i32);
        self.clip = (x0, y0, x1 as i16, y1 as i16);
    }

    /// 裁剪窗口恢复为整屏。
    pub fn reset_clip(&mut self) {
        self.clip = (0, 0, WIDTH, HEIGHT);
    }

    /// 当前裁剪窗口 `(x0, y0, x1, y1)`。
    pub fn clip(&self) -> (i16, i16, i16, i16) {
        self.clip
    }

    /// 设置文字字体，同时刷新上升量与下降量。
    pub fn set_font(&mut self, font: &'static Font) {
        self.font = Some(font);
        self.ascent = font.ascent();
        self.descent = font.descent();
    }

    /// 当前字体上升量（像素）。
    pub fn ascent(&self) -> i16 {
        self.ascent
    }

    /// 当前字体下降量（像素，负值）。
    pub fn descent(&self) -> i16 {
        self.descent
    }

    /// 按绘图模式写入一个源像素。
    fn plot(&mut self, x: i16, y: i16, on: bool) {
        let (x0, y0, x1, y1) = self.clip;
        if x < x0 || x >= x1 || y < y0 || y >= y1 || !on {
            return;
        }
        let index = x as usize + (y as usize >> 3) * WIDTH as usize;
        let mask = 1u8 << (y & 7);
        match self.mode {
            DrawMode::Set => self.buf[index] |= mask,
            DrawMode::Clear => self.buf[index] &= !mask,
            DrawMode::Xor => self.buf[index] ^= mask,
        }
    }

    /// 写入一个源像素：`on` 为 `false` 时按背景透明处理，直接跳过。
    pub(crate) fn plot_source(&mut self, x: i16, y: i16, on: bool) {
        self.plot(x, y, on);
    }

    /// 绘制一个像素。
    pub fn draw_pixel(&mut self, x: i16, y: i16) {
        self.plot(x, y, true);
    }

    /// 从 `(x, y)` 向右绘制 `width` 个像素。
    pub fn draw_hline(&mut self, x: i16, y: i16, width: u16) {
        if width == 0 || width > i16::MAX as u16 {
            return;
        }
        self.fill_row(x as i32, x as i32 + width as i32, y);
    }

    /// 从 `(x, y)` 向下绘制 `height` 个像素。
    pub fn draw_vline(&mut self, x: i16, y: i16, height: u16) {
        if height == 0 || height > i16::MAX as u16 {
            return;
        }
        let (_, y0, _, y1) = self.clip;
        if x < self.clip.0 || x >= self.clip.2 {
            return;
        }
        let begin = (y as i32).max(y0 as i32);
        let end = (y as i32 + height as i32).min(y1 as i32);
        for py in begin..end {
            self.plot(x, py as i16, true);
        }
    }

    /// 把 `[left, right)` 这一行按裁剪窗口收缩后画出来。
    fn fill_row(&mut self, left: i32, right: i32, y: i16) {
        let (x0, y0, x1, y1) = self.clip;
        if y < y0 || y >= y1 || right <= x0 as i32 || left >= x1 as i32 {
            return;
        }
        let begin = left.max(x0 as i32);
        let end = right.min(x1 as i32);
        for px in begin..end {
            self.plot(px as i16, y, true);
        }
    }

    /// 绘制包含两个端点的直线（对称 Bresenham）。
    pub fn draw_line(&mut self, x0: i16, y0: i16, x1: i16, y1: i16) {
        if !self.clip_intersects(
            x0.min(x1) as i32,
            y0.min(y1) as i32,
            x0.max(x1) as i32 + 1,
            y0.max(y1) as i32 + 1,
        ) {
            return;
        }
        let mut x = x0 as i32;
        let mut y = y0 as i32;
        let dx = (x1 as i32 - x0 as i32).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 as i32 - y0 as i32).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut error = dx + dy;
        loop {
            self.plot(x as i16, y as i16, true);
            if x == x1 as i32 && y == y1 as i32 {
                break;
            }
            let doubled = error * 2;
            if doubled >= dy {
                error += dy;
                x += sx;
            }
            if doubled <= dx {
                error += dx;
                y += sy;
            }
        }
    }

    /// 绘制精确宽高的矩形边框。
    pub fn draw_frame(&mut self, x: i16, y: i16, width: u16, height: u16) {
        if !self.rect_visible(x, y, width, height) {
            return;
        }
        self.draw_hline(x, y, width);
        if height > 1 {
            self.draw_hline(x, y + height as i16 - 1, width);
        }
        if height > 2 {
            self.draw_vline(x, y + 1, height - 2);
            if width > 1 {
                self.draw_vline(x + width as i16 - 1, y + 1, height - 2);
            }
        }
    }

    /// 绘制精确宽高的实心矩形。
    pub fn draw_box(&mut self, x: i16, y: i16, width: u16, height: u16) {
        if !self.rect_visible(x, y, width, height) {
            return;
        }
        let (_, y0, _, y1) = self.clip;
        let begin = (y as i32).max(y0 as i32);
        let end = (y as i32 + height as i32).min(y1 as i32);
        for py in begin..end {
            self.fill_row(x as i32, x as i32 + width as i32, py as i16);
        }
    }

    /// 绘制圆角矩形边框。
    pub fn draw_rframe(&mut self, x: i16, y: i16, width: u16, height: u16, radius: u16) {
        let Some((radius, top, bottom)) = self.rounded_setup(x, y, width, height, radius) else {
            return;
        };
        if radius == 0 {
            self.draw_frame(x, y, width, height);
            return;
        }
        for py in top..bottom {
            let local_y = py - y;
            let inset = rounded_inset(height, radius, local_y);
            let left = x as i32 + inset as i32;
            let right = x as i32 + width as i32 - 1 - inset as i32;
            if local_y == 0 || local_y == height as i16 - 1 {
                self.fill_row(left, right + 1, py);
                continue;
            }
            let previous = rounded_inset(height, radius, local_y - 1);
            let previous_left = x as i32 + previous as i32;
            let previous_right = x as i32 + width as i32 - 1 - previous as i32;
            self.outline_row(left, previous_left, right, previous_right, py);
        }
    }

    /// 绘制实心圆角矩形。
    pub fn draw_rbox(&mut self, x: i16, y: i16, width: u16, height: u16, radius: u16) {
        let Some((radius, top, bottom)) = self.rounded_setup(x, y, width, height, radius) else {
            return;
        };
        if radius == 0 {
            self.draw_box(x, y, width, height);
            return;
        }
        for py in top..bottom {
            let inset = rounded_inset(height, radius, py - y);
            self.fill_row(
                x as i32 + inset as i32,
                x as i32 + width as i32 - inset as i32,
                py,
            );
        }
    }

    /// 圆角矩形的公共前置检查：返回收缩后的半径与可见行区间。
    fn rounded_setup(
        &mut self,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        radius: u16,
    ) -> Option<(u16, i16, i16)> {
        if width == 0 || height == 0 || width > i16::MAX as u16 || height > i16::MAX as u16 {
            return None;
        }
        let (x0, y0, x1, y1) = self.clip;
        if !self.clip_intersects(
            x as i32,
            y as i32,
            x as i32 + width as i32,
            y as i32 + height as i32,
        ) {
            return None;
        }
        let radius = radius.min((width.min(height) - 1) / 2);
        let top = (y as i32).max(y0 as i32) as i16;
        let bottom = (y as i32 + height as i32).min(y1 as i32) as i16;
        let _ = (x0, x1);
        Some((radius, top, bottom))
    }

    /// 画出圆角轮廓在相邻两行之间的新增像素区间。
    fn outline_row(
        &mut self,
        left: i32,
        previous_left: i32,
        right: i32,
        previous_right: i32,
        y: i16,
    ) {
        let (left_begin, left_end) = span_edge(left, previous_left);
        let (right_begin, right_end) = span_edge(right, previous_right);
        if left_end + 1 >= right_begin {
            self.fill_row(left_begin, right_end + 1, y);
        } else {
            self.fill_row(left_begin, left_end + 1, y);
            self.fill_row(right_begin, right_end + 1, y);
        }
    }

    /// 绘制圆周。
    pub fn draw_circle(&mut self, x: i16, y: i16, radius: u16) {
        if radius as i16 > WIDTH.max(HEIGHT) || !self.curve_visible(x, y, radius, radius) {
            return;
        }
        self.midpoint_circle(x, y, radius);
    }

    /// 绘制实心圆。
    pub fn draw_disc(&mut self, x: i16, y: i16, radius: u16) {
        if radius as i16 > WIDTH.max(HEIGHT) || !self.curve_visible(x, y, radius, radius) {
            return;
        }
        self.fill_ellipse(x, y, radius, radius);
    }

    /// 中点圆算法的八分对称绘制。
    fn midpoint_circle(&mut self, cx: i16, cy: i16, radius: u16) {
        let mut px = radius as i32;
        let mut py = 0i32;
        let mut error = 1 - px;
        loop {
            self.plot_circle_points(cx, cy, px, py);
            py += 1;
            if error < 0 {
                error += 2 * py + 1;
            } else {
                px -= 1;
                error += 2 * (py - px) + 1;
            }
            if px < py {
                break;
            }
        }
    }

    /// 把一个八分区点镜像到整圆。
    fn plot_circle_points(&mut self, cx: i16, cy: i16, x: i32, y: i32) {
        let mut point = |dx: i32, dy: i32| {
            self.plot((cx as i32 + dx) as i16, (cy as i32 + dy) as i16, true);
        };
        point(x, y);
        if y != 0 {
            point(x, -y);
        }
        if x != 0 {
            point(-x, y);
            if y != 0 {
                point(-x, -y);
            }
        }
        if x != y {
            point(y, x);
            point(y, -x);
            if y != 0 {
                point(-y, x);
                point(-y, -x);
            }
        }
    }

    /// 逐扫描行填充椭圆。
    fn fill_ellipse(&mut self, cx: i16, cy: i16, radius_x: u16, radius_y: u16) {
        for offset in 0..=radius_y as i32 {
            let px = ellipse_x(radius_x, radius_y, offset);
            let width = (2 * px + 1) as u16;
            let left = cx as i32 - px as i32;
            self.fill_row(left, left + width as i32, (cy as i32 + offset) as i16);
            if offset != 0 {
                self.fill_row(left, left + width as i32, (cy as i32 - offset) as i16);
            }
        }
    }

    /// 绘制 XBM（行优先、字节内 LSB 在左）。
    pub fn draw_xbm(&mut self, x: i16, y: i16, width: u16, height: u16, bitmap: &[u8]) {
        let extent = WIDTH.max(HEIGHT) as u16;
        if width == 0 || height == 0 || width > extent || height > extent {
            return;
        }
        if !self.clip_intersects(
            x as i32,
            y as i32,
            x as i32 + width as i32,
            y as i32 + height as i32,
        ) {
            return;
        }
        let stride = (width as usize).div_ceil(8);
        for row in 0..height as usize {
            for col in 0..width as usize {
                let index = row * stride + (col >> 3);
                let Some(byte) = bitmap.get(index) else {
                    return;
                };
                let on = byte & (1u8 << (col & 7)) != 0;
                self.plot(x + col as i16, y + row as i16, on);
            }
        }
    }

    /// 绘制 UTF-8 单行字符串，返回累计 advance。
    pub fn draw_text(&mut self, x: i16, y: i16, text: &str) -> i16 {
        let Some(font) = self.font else {
            return 0;
        };
        font.draw_text(self, x, y, text)
    }

    /// 计算字符串可见像素宽度。
    pub fn text_width(&self, text: &str) -> u16 {
        self.font.map(|font| font.text_width(text)).unwrap_or(0)
    }

    /// 半开包围盒与裁剪窗口求交；有交集时把包围盒收缩到窗口内。
    fn clip_intersects(&self, mut x0: i32, mut y0: i32, mut x1: i32, mut y1: i32) -> bool {
        let (cx0, cy0, cx1, cy1) = self.clip;
        x0 = x0.max(cx0 as i32);
        y0 = y0.max(cy0 as i32);
        x1 = x1.min(cx1 as i32);
        y1 = y1.min(cy1 as i32);
        x0 < x1 && y0 < y1
    }

    /// 校验矩形尺寸并判断是否与裁剪窗口相交。
    fn rect_visible(&mut self, x: i16, y: i16, width: u16, height: u16) -> bool {
        width != 0
            && height != 0
            && width <= i16::MAX as u16
            && height <= i16::MAX as u16
            && self.clip_intersects(
                x as i32,
                y as i32,
                x as i32 + width as i32,
                y as i32 + height as i32,
            )
    }

    /// 校验曲线半径并判断包围盒是否可见。
    fn curve_visible(&self, x: i16, y: i16, radius_x: u16, radius_y: u16) -> bool {
        self.clip_intersects(
            x as i32 - radius_x as i32,
            y as i32 - radius_y as i32,
            x as i32 + radius_x as i32 + 1,
            y as i32 + radius_y as i32 + 1,
        )
    }
}

/// 圆角矩形某一行左右两侧的缩进量。
fn rounded_inset(height: u16, radius: u16, local_y: i16) -> i16 {
    let radius_i = radius as i16;
    if local_y < radius_i {
        return radius_i - ellipse_x(radius, radius, radius_i as i32 - local_y as i32);
    }
    if local_y >= height as i16 - radius_i {
        let dy = local_y - (height as i16 - 1 - radius_i);
        return radius_i - ellipse_x(radius, radius, dy as i32);
    }
    0
}

/// 椭圆在纵向偏移 `dy` 处仍在内部的横向半径。
fn ellipse_x(radius_x: u16, radius_y: u16, dy: i32) -> i16 {
    let rx = radius_x as i32;
    let ry = radius_y as i32;
    let rx2 = rx * rx;
    let ry2 = ry * ry;
    let limit = rx2 * ry2;
    let dy2 = dy * dy;
    let mut px = rx;
    while px > 0 && px * px * ry2 + dy2 * rx2 > limit {
        px -= 1;
    }
    px as i16
}

/// 相邻两行圆角轮廓之间的新增像素区间。
fn span_edge(current: i32, previous: i32) -> (i32, i32) {
    match current.cmp(&previous) {
        core::cmp::Ordering::Less => (current, previous - 1),
        core::cmp::Ordering::Greater => (previous + 1, current),
        core::cmp::Ordering::Equal => (current, current),
    }
}
