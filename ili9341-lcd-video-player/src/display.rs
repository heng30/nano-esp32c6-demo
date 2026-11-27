//! 8080-16 位并口的屏驱动：GPIO 直写 `out` 寄存器打时序。
//!
//! 并口没有通用外设可用，只能 GPIO 打时序：数据线连同 DC/WR 都在 GPIO0–31 同一组，
//! 所以直接写 `out` 寄存器整体覆盖，一个像素两次寄存器写。掩码表放 RAM，
//! 放 flash 时每像素多两次 XIP 读。
//!
//! 为什么不用 PARL_IO（本目录实测过，结论见 README）：它每次传输开头会多打约 68 个
//! WR 脉冲（真实锁存，会把 GRAM 写指针前推），而单帧长度寄存器是 u16，一帧得分 3 段推，
//! 于是画面三处 68 像素错位；校准帧尾只能消掉逐帧爬行，消不掉段间错位。
//! I2S 并行那条路在 esp-hal 1.2 里是 `#[cfg(esp32)]`，C6 上不存在。

use defmt::info;
use esp_hal::{delay::Delay, gpio::Output};

/// 数据线 GPIO 编号，下标即位序，不连续。
const DATA_PINS: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 18, 19, 20, 21, 22];
/// RS / D/CX：低=命令，高=数据。
const DC_PIN: u8 = 15;
/// WR：上升沿锁存。
const WR_PIN: u8 = 23;

/// GPIO 外设基址（TRM 里的 GPIO 基址）。
const GPIO_BASE: usize = 0x6009_1000;
/// GPIO_OUT_REG：整体写输出电平。
const GPIO_OUT: usize = GPIO_BASE + 0x04;
/// 全部数据线的掩码。
const DATA_MASK: u32 = data_mask();
/// 数据线 + DC + WR，整体写 out 时需要先清掉这些位。
const CTRL_MASK: u32 = DATA_MASK | (1 << DC_PIN) | (1 << WR_PIN);

/// 屏宽（像素）。
pub(crate) const WIDTH: usize = 320;
/// 屏高（像素）。
pub(crate) const HEIGHT: usize = 240;

/// 软件复位。
const CMD_RESET: u8 = 0x01;
/// 睡眠退出。
const CMD_SLEEP_OUT: u8 = 0x11;
/// 显示开。
const CMD_DISPLAY_ON: u8 = 0x29;
/// 内存写。
const CMD_RAM_WRITE: u8 = 0x2c;
/// 内存访问方向。
const CMD_MADCTL: u8 = 0x36;
/// 像素格式。
const CMD_COLMOD: u8 = 0x3a;
/// 列地址窗口。
const CMD_COLUMN_SET: u8 = 0x2a;
/// 行地址窗口。
const CMD_PAGE_SET: u8 = 0x2b;
/// 内存访问方向：横屏、RGB 顺序。
const MADCTL_LANDSCAPE: u8 = 0x28;
/// 像素格式：16 位 RGB565。
const COLMOD_RGB565: u8 = 0x55;

/// 低 8 位像素值对应的置位掩码，去掉逐位循环。必须落在 RAM。
#[esp_hal::ram]
static SET_LO: [u32; 256] = mask_table(0);
/// 高 8 位像素值对应的置位掩码，同样落 RAM。
#[esp_hal::ram]
static SET_HI: [u32; 256] = mask_table(8);

const fn data_mask() -> u32 {
    let mut mask = 0u32;
    let mut i = 0;
    while i < DATA_PINS.len() {
        mask |= 1 << DATA_PINS[i];
        i += 1;
    }
    mask
}

/// 生成 `base..base+8` 这 8 位数据线的置位掩码表。
const fn mask_table(base: usize) -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut value = 0;
    while value < 256 {
        let mut mask = 0u32;
        let mut bit = 0;
        while bit < 8 {
            if value & (1 << bit) != 0 {
                mask |= 1 << DATA_PINS[base + bit];
            }
            bit += 1;
        }
        table[value] = mask;
        value += 1;
    }
    table
}

/// 8080-16 位并口：16 根数据线 + RS(DC) + WR。CS 接地、RD 接 3V3，只写不读。
pub(crate) struct Display {
    /// 只负责把引脚配成输出并持有所有权，电平由下面的寄存器直写决定。
    _data: [Output<'static>; 16],
    /// RS/D/CX。
    _dc: Output<'static>,
    /// WR。
    _wr: Output<'static>,
    /// `out` 寄存器的影子值：整体写会连带别的引脚一起覆盖，所以先缓存一份。
    shadow: u32,
}

impl Display {
    /// 屏初始化：复位、横屏、RGB565、开显示，最后设整屏窗口并进入内存写。
    ///
    /// 返回后 DC 已拉高，`draw_frame` 只送像素。
    pub(crate) fn new(
        data: [Output<'static>; 16],
        dc: Output<'static>,
        wr: Output<'static>,
        delay: &Delay,
    ) -> Self {
        // 引脚已在 esp-hal 里配成输出，这里读一次当前 out 值当影子
        let shadow = unsafe { (GPIO_OUT as *const u32).read_volatile() };
        let mut this = Self {
            _data: data,
            _dc: dc,
            _wr: wr,
            shadow,
        };

        this.command(CMD_RESET, &[]);
        delay.delay_millis(120);
        this.command(CMD_MADCTL, &[MADCTL_LANDSCAPE]);
        this.command(CMD_COLMOD, &[COLMOD_RGB565]);
        this.command(CMD_SLEEP_OUT, &[]);
        delay.delay_millis(5);
        this.command(CMD_DISPLAY_ON, &[]);

        // 整屏窗口 + 内存写：之后 draw_frame 送的都是连续像素，写指针自己递增
        let (x1, y1) = ((WIDTH - 1) as u16, (HEIGHT - 1) as u16);
        this.command(CMD_COLUMN_SET, &[0, 0, (x1 >> 8) as u8, x1 as u8]);
        this.command(CMD_PAGE_SET, &[0, 0, (y1 >> 8) as u8, y1 as u8]);
        this.command(CMD_RAM_WRITE, &[]);

        info!("ili9341 ready {}x{}", WIDTH, HEIGHT);
        this
    }

    /// 一次并口写：摆好数据与 DC，再打一个 WR 脉冲。
    #[inline(always)]
    fn write(&mut self, value: u16, is_data: bool) {
        let mut set = SET_LO[(value & 0xff) as usize] | SET_HI[(value >> 8) as usize];
        if is_data {
            set |= 1 << DC_PIN;
        }

        let latched = (self.shadow & !CTRL_MASK) | set;
        unsafe {
            (GPIO_OUT as *mut u32).write_volatile(latched);
            (GPIO_OUT as *mut u32).write_volatile(latched | 1 << WR_PIN);
        }
    }

    /// 发一条命令 + 参数：16 位并口下命令与参数都只走 D[7:0]，高字节为 0。
    fn command(&mut self, cmd: u8, args: &[u8]) {
        self.write(u16::from(cmd), false);
        for arg in args {
            self.write(u16::from(*arg), true);
        }
    }

    /// 把整屏像素一次推出去（窗口已在 `new` 里设好，写指针自己递增）。
    pub(crate) fn draw_frame(&mut self, pixels: &[u16]) {
        for pixel in pixels {
            self.write(*pixel, true);
        }
    }
}
