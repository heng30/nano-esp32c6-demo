//! 抓屏帧 → 用 `fast_image_resize` 按 Box 滤波缩到板子上报的分辨率，再按 16×16 分块找脏块。
//!
//! 用 Box（面积平均）而不是线性/三次插值：7.7 倍缩小下插值采样会让等宽字体的一像素竖线整条消失。

use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer, images::Image};

/// 一个 tile 的边长（像素），与固件一致。
pub const TILE: usize = 16;

/// 一块要发给板子的像素。
#[derive(Clone, Copy)]
pub struct Tile {
    /// tile 列号。
    pub col: u8,
    /// tile 行号。
    pub row: u8,
    /// 每边的源像素数（`TILE / scale`），`pixels` 只用前 `arm * arm` 个。
    pub arm: u8,
    /// 16×16 的 RGB565 像素，行优先；放大时只用左上 `arm × arm`。
    pub pixels: [u16; TILE * TILE],
}

/// 缩放器、缩放后的帧、当前帧、上一帧与脏块列表（块下标 = 行 × `20` + 列）。
pub struct Frame {
    /// 复用的缩放器，内部临时缓冲只在换源尺寸时才重建。
    resizer: Resizer,
    /// 缩放参数。
    options: ResizeOptions,
    /// 采样倍数：抓屏缩到 `1/scale`，每块只采 `(16/scale)²` 个源像素。
    scale: usize,
    /// 每边的源像素数（`TILE / scale`）。
    arm: usize,
    /// 每行的 tile 数。
    tiles_x: usize,
    /// tile 的行数。
    tiles_y: usize,
    /// 缩放结果：`面板尺寸 / scale` 的 RGBA。
    scaled: Image<'static>,
    /// 缩放后的像素宽度（行跨距）。
    width: usize,
    cur: Vec<u16>,
    prev: Vec<u16>,
    dirty: Vec<usize>,
    /// 下一次 update 把整屏算脏（发送口挂掉后补屏用）。
    dirty_all: bool,
}

impl Frame {
    /// `width` × `height` 是板子上报的面板分辨率，`scale` 是它的采样倍数：
    /// 抓屏缩到 `1/scale`，每块只采 `(16/scale)²` 个源像素，板子再铺成 `scale × scale`。
    pub fn new(width: usize, height: usize, scale: usize) -> Self {
        let scale = scale.clamp(1, TILE);
        let arm = TILE / scale;
        let (tiles_x, tiles_y) = (width / TILE, height / TILE);
        let (width, height) = (width / scale, height / scale);
        // 抓屏是 Xrgb8888，X 字节不可信（常常是 0），必须关掉 alpha 预乘，否则整屏会变黑
        let options = ResizeOptions {
            mul_div_alpha: false,
            ..ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Box))
        };
        Self {
            resizer: Resizer::new(),
            options,
            scale,
            arm,
            scaled: Image::new(width as u32, height as u32, PixelType::U8x4),
            width,
            tiles_x,
            tiles_y,
            cur: vec![0; width * height],
            prev: vec![0; width * height],
            dirty: Vec::with_capacity(tiles_x * tiles_y),
            dirty_all: false,
        }
    }

    /// 把 `rgba`（RGBA8888，`width` × `height`）整幅缩放成 `1/scale` 大小，并刷新脏块列表。
    ///
    /// 缓冲不够一张整帧或缩放失败时返回 `false`，不改动任何状态。
    pub fn update(&mut self, rgba: &mut [u8], width: usize, height: usize) -> bool {
        let (width, height) = (width as u32, height as u32);
        if width == 0 || height == 0 || rgba.len() < width as usize * height as usize * 4 {
            return false;
        }

        // 借用抓屏缓冲，不复制整帧
        let Ok(source) = Image::from_slice_u8(width, height, rgba, PixelType::U8x4) else {
            return false;
        };
        if self
            .resizer
            .resize(&source, &mut self.scaled, &self.options)
            .is_err()
        {
            return false;
        }

        for (pixel, out) in self
            .scaled
            .buffer()
            .as_chunks::<4>()
            .0
            .iter()
            .zip(self.cur.iter_mut())
        {
            *out = rgb565(pixel[0], pixel[1], pixel[2]);
        }

        self.dirty.clear();
        let force = self.dirty_all;
        self.dirty_all = false;
        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                if force || self.tile_changed(tx, ty) {
                    self.dirty.push(ty * self.tiles_x + tx);
                }
            }
        }
        self.prev.copy_from_slice(&self.cur);
        true
    }

    /// 当前采样倍数。
    pub fn scale(&self) -> usize {
        self.scale
    }

    /// 上一帧之后变化的块下标。
    pub fn dirty(&self) -> &[usize] {
        &self.dirty
    }

    /// 下一次 update 把整屏都算脏（发送口挂掉后补屏用）。
    pub fn invalidate(&mut self) {
        self.dirty_all = true;
    }

    /// 取第 `index` 块，像素按行优先展开成连续 `arm * arm` 个。
    pub fn tile(&self, index: usize) -> Tile {
        let (col, row) = (index % self.tiles_x, index / self.tiles_x);
        let arm = self.arm;
        let mut pixels = [0u16; TILE * TILE];
        for line in 0..arm {
            let from = (row * arm + line) * self.width + col * arm;
            let to = line * arm;
            pixels[to..to + arm].copy_from_slice(&self.cur[from..from + arm]);
        }
        Tile {
            col: col as u8,
            row: row as u8,
            arm: arm as u8,
            pixels,
        }
    }

    fn tile_changed(&self, tx: usize, ty: usize) -> bool {
        let arm = self.arm;
        (0..arm).any(|line| {
            let from = (ty * arm + line) * self.width + tx * arm;
            self.cur[from..from + arm] != self.prev[from..from + arm]
        })
    }
}

/// RGB888 → RGB565。
fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张左右对半（左黑右白）或全黑的 RGBA 图。
    fn source(width: usize, height: usize, right_half_white: bool) -> Vec<u8> {
        let mut rgba = vec![0u8; width * height * 4];
        for row in 0..height {
            for col in 0..width {
                let at = (row * width + col) * 4;
                let value = if right_half_white && col >= width / 2 {
                    0xFF
                } else {
                    0
                };
                rgba[at..at + 4].copy_from_slice(&[value, value, value, 0]);
            }
        }
        rgba
    }

    #[test]
    fn scales_whole_frame_into_the_panel() {
        let mut frame = Frame::new(320, 240, 1);
        let mut rgba = source(640, 480, true);
        assert!(frame.update(&mut rgba, 640, 480));

        assert_eq!(frame.cur[0], 0x0000);
        assert_eq!(frame.cur[320 * 240 - 1], 0xFFFF);
        // prev 全零，所以只有右半（白）那几列算脏块
        assert_eq!(frame.dirty().len(), 20 / 2 * 15);
        assert!(!frame.dirty().contains(&0));
        assert!(frame.dirty().contains(&(20 - 1)));
    }

    #[test]
    fn ignores_alpha_channel() {
        // Xrgb 抓屏的 alpha 字节是 0，关闭 alpha 预乘后颜色不能被它吃掉
        let mut frame = Frame::new(320, 240, 1);
        let mut rgba = source(640, 480, true);
        assert!(frame.update(&mut rgba, 640, 480));

        assert_eq!(frame.cur[320 * 240 - 1], 0xFFFF);
    }

    #[test]
    fn second_identical_frame_has_no_dirty_tile() {
        let mut frame = Frame::new(320, 240, 1);
        let mut rgba = source(640, 480, true);

        assert!(frame.update(&mut rgba, 640, 480));
        assert!(frame.update(&mut rgba, 640, 480));
        assert!(frame.dirty().is_empty());
    }

    #[test]
    fn invalidate_marks_every_tile_dirty() {
        let mut frame = Frame::new(320, 240, 1);
        let mut rgba = source(640, 480, true);
        assert!(frame.update(&mut rgba, 640, 480));
        assert!(!frame.dirty().is_empty());

        frame.update(&mut rgba, 640, 480);
        assert!(frame.dirty().is_empty());

        frame.invalidate();
        assert!(frame.update(&mut rgba, 640, 480));
        assert_eq!(frame.dirty().len(), 20 * 15);
    }

    #[test]
    fn rejects_short_buffer() {
        let mut frame = Frame::new(320, 240, 1);
        assert!(!frame.update(&mut [], 640, 480));
        let mut rgba = source(4, 4, false);
        assert!(!frame.update(&mut rgba, 0, 4));
    }

    #[test]
    fn tile_extracts_contiguous_rows() {
        let mut frame = Frame::new(320, 240, 1);
        let mut rgba = source(640, 480, true);
        assert!(frame.update(&mut rgba, 640, 480));

        let tile = frame.tile(20 - 1);
        assert_eq!((tile.col, tile.row), ((20 - 1) as u8, 0));
        assert_eq!(tile.pixels[TILE * TILE - 1], 0xFFFF);
    }
}
