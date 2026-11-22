//! 伪 3D 隧道：径向环 + 八向扇区，靠整数运算凑出纵深感。

use slint::platform::software_renderer::Rgb565Pixel;

use super::{Canvas, HEIGHT, WIDTH};

/// 每帧往前推进的环数。
const SPEED: i32 = 3;
/// 调色板长度。
const LEVELS: usize = 16;

/// 隧道配色：由深蓝过渡到青白。
const PALETTE: [u16; LEVELS] = [
    0x0010, 0x0030, 0x0050, 0x0070, 0x0090, 0x00b0, 0x00d0, 0x00f0, 0x08f8, 0x0cf8, 0x2ff8, 0x6ff8,
    0xaff8, 0xefff, 0xffff, 0x7fdf,
];

/// 向内收缩的隧道。
pub struct Tunnel;

impl Canvas for Tunnel {
    fn draw(&mut self, px: &mut [Rgb565Pixel], t_ms: u32) {
        let cx = (WIDTH / 2) as i32;
        let cy = (HEIGHT / 2) as i32;
        let phase = (t_ms / 24) as i32 * SPEED;

        for y in 0..HEIGHT {
            let dy = y as i32 - cy;
            for x in 0..WIDTH {
                let dx = x as i32 - cx;
                let r2 = dx * dx + dy * dy;

                // 越靠近中心 r2 越小，环号跳得越快，透视收缩感就出来了
                let ring = (phase + 6000 / (r2 / 48 + 1)) as usize;
                // 八个扇区：不用 atan2，只看符号和 |dx|、|dy| 的大小关系
                let mut sector = (dx > 0) as usize | ((dy > 0) as usize) << 1;
                if dx.abs() > dy.abs() {
                    sector |= 4;
                }

                px[y * WIDTH + x] = Rgb565Pixel(PALETTE[(ring + sector * 2) % LEVELS]);
            }
        }
    }
}
