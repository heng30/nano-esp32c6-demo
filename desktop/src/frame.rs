//! 抓屏帧 → 用 `fast_image_resize` 按 Box 滤波缩到 320×240 的 RGB565，再按 16×16 分块找脏块。
//!
//! 用 Box（面积平均）而不是线性/三次插值：7.7 倍缩小下插值采样会让等宽字体的一像素竖线整条消失。

use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer, images::Image};

/// 目标宽度（像素），与固件一致。
pub const WIDTH: usize = 320;
/// 目标高度（像素），与固件一致。
pub const HEIGHT: usize = 240;
/// 一个 tile 的边长（像素）。
pub const TILE: usize = 16;
/// 每行的 tile 数。
pub const TILES_X: usize = WIDTH / TILE;
/// tile 的行数。
pub const TILES_Y: usize = HEIGHT / TILE;

/// 一块要发给板子的像素。
#[derive(Clone, Copy)]
pub struct Tile {
    /// tile 列号。
    pub col: u8,
    /// tile 行号。
    pub row: u8,
    /// 16×16 的 RGB565 像素，行优先。
    pub pixels: [u16; TILE * TILE],
}

/// 缩放器、缩放后的帧、当前帧、上一帧与脏块列表（块下标 = 行 × `TILES_X` + 列）。
pub struct Frame {
    /// 复用的缩放器，内部临时缓冲只在换源尺寸时才重建。
    resizer: Resizer,
    /// 缩放参数。
    options: ResizeOptions,
    /// 缩放结果：320×240 的 RGBA。
    scaled: Image<'static>,
    cur: Vec<u16>,
    prev: Vec<u16>,
    dirty: Vec<usize>,
    /// 下一次 update 把整屏算脏（发送口挂掉后补屏用）。
    dirty_all: bool,
}

impl Frame {
    pub fn new() -> Self {
        // 抓屏是 Xrgb8888，X 字节不可信（常常是 0），必须关掉 alpha 预乘，否则整屏会变黑
        let options = ResizeOptions {
            mul_div_alpha: false,
            ..ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Box))
        };
        Self {
            resizer: Resizer::new(),
            options,
            scaled: Image::new(WIDTH as u32, HEIGHT as u32, PixelType::U8x4),
            cur: vec![0; WIDTH * HEIGHT],
            prev: vec![0; WIDTH * HEIGHT],
            dirty: Vec::with_capacity(TILES_X * TILES_Y),
            dirty_all: false,
        }
    }

    /// 把 `rgba`（RGBA8888，`width` × `height`）整幅缩放成 320×240，并刷新脏块列表。
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
        for ty in 0..TILES_Y {
            for tx in 0..TILES_X {
                if force || self.tile_changed(tx, ty) {
                    self.dirty.push(ty * TILES_X + tx);
                }
            }
        }
        self.prev.copy_from_slice(&self.cur);
        true
    }

    /// 上一帧之后变化的块下标。
    pub fn dirty(&self) -> &[usize] {
        &self.dirty
    }

    /// 下一次 update 把整屏都算脏（发送口挂掉后补屏用）。
    pub fn invalidate(&mut self) {
        self.dirty_all = true;
    }

    /// 取第 `index` 块，像素按行优先展开成连续 256 个。
    pub fn tile(&self, index: usize) -> Tile {
        let (col, row) = (index % TILES_X, index / TILES_X);
        let mut pixels = [0u16; TILE * TILE];
        for line in 0..TILE {
            let from = (row * TILE + line) * WIDTH + col * TILE;
            let to = line * TILE;
            pixels[to..to + TILE].copy_from_slice(&self.cur[from..from + TILE]);
        }
        Tile {
            col: col as u8,
            row: row as u8,
            pixels,
        }
    }

    fn tile_changed(&self, tx: usize, ty: usize) -> bool {
        (0..TILE).any(|line| {
            let from = (ty * TILE + line) * WIDTH + tx * TILE;
            self.cur[from..from + TILE] != self.prev[from..from + TILE]
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
        let mut frame = Frame::new();
        let mut rgba = source(640, 480, true);
        assert!(frame.update(&mut rgba, 640, 480));

        assert_eq!(frame.cur[0], 0x0000);
        assert_eq!(frame.cur[WIDTH * HEIGHT - 1], 0xFFFF);
        // prev 全零，所以只有右半（白）那几列算脏块
        assert_eq!(frame.dirty().len(), TILES_X / 2 * TILES_Y);
        assert!(!frame.dirty().contains(&0));
        assert!(frame.dirty().contains(&(TILES_X - 1)));
    }

    #[test]
    fn ignores_alpha_channel() {
        // Xrgb 抓屏的 alpha 字节是 0，关闭 alpha 预乘后颜色不能被它吃掉
        let mut frame = Frame::new();
        let mut rgba = source(640, 480, true);
        assert!(frame.update(&mut rgba, 640, 480));

        assert_eq!(frame.cur[WIDTH * HEIGHT - 1], 0xFFFF);
    }

    #[test]
    fn second_identical_frame_has_no_dirty_tile() {
        let mut frame = Frame::new();
        let mut rgba = source(640, 480, true);

        assert!(frame.update(&mut rgba, 640, 480));
        assert!(frame.update(&mut rgba, 640, 480));
        assert!(frame.dirty().is_empty());
    }

    #[test]
    fn invalidate_marks_every_tile_dirty() {
        let mut frame = Frame::new();
        let mut rgba = source(640, 480, true);
        assert!(frame.update(&mut rgba, 640, 480));
        assert!(!frame.dirty().is_empty());

        frame.update(&mut rgba, 640, 480);
        assert!(frame.dirty().is_empty());

        frame.invalidate();
        assert!(frame.update(&mut rgba, 640, 480));
        assert_eq!(frame.dirty().len(), TILES_X * TILES_Y);
    }

    #[test]
    fn rejects_short_buffer() {
        let mut frame = Frame::new();
        assert!(!frame.update(&mut [], 640, 480));
        let mut rgba = source(4, 4, false);
        assert!(!frame.update(&mut rgba, 0, 4));
    }

    #[test]
    fn tile_extracts_contiguous_rows() {
        let mut frame = Frame::new();
        let mut rgba = source(640, 480, true);
        assert!(frame.update(&mut rgba, 640, 480));

        let tile = frame.tile(TILES_X - 1);
        assert_eq!((tile.col, tile.row), ((TILES_X - 1) as u8, 0));
        assert_eq!(tile.pixels[TILE * TILE - 1], 0xFFFF);
    }
}
