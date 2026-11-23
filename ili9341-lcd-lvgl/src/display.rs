//! 8080-16 位并口与屏初始化（与 `../ili9341-lcd-slint/src/display.rs` 同一份实现）。
//!
//! 并口没有通用外设可用，只能 GPIO 打时序：数据线连同 DC/WR 都在 GPIO0–31 同一组，
//! 所以直接写 `out` 寄存器整体覆盖，一个像素两次寄存器写。

use defmt::info;
use display_interface::{DataFormat, DisplayError, WriteOnlyDataCommand};
use esp_hal::{delay::Delay, gpio::Output};
use ili9341::{DisplaySize240x320, Ili9341, Orientation};

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

/// 低 8 位像素值对应的置位掩码，去掉逐位循环。
static SET_LO: [u32; 256] = mask_table(0);
/// 高 8 位像素值对应的置位掩码。
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

/// 屏的 RST 与板子 EN 共复位，没有独立 GPIO。`ili9341` 初始化时会拉低再拉高一次，
/// 这里用空脚顶替，避免占用一个不存在的引脚。
pub(crate) struct NoReset;

impl embedded_hal::digital::ErrorType for NoReset {
    type Error = core::convert::Infallible;
}

impl embedded_hal::digital::OutputPin for NoReset {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// 8080-16 位并口：16 根数据线 + RS(DC) + WR。CS 接地、RD 接 3V3，只写不读。
pub(crate) struct Bus {
    /// 只负责把引脚配成输出并持有所有权，电平由下面的寄存器直写决定。
    _data: [Output<'static>; 16],
    /// RS/D/CX。
    _dc: Output<'static>,
    /// WR。
    _wr: Output<'static>,
    /// `out` 寄存器的影子值：整体写会连带别的引脚一起覆盖，所以先缓存一份。
    shadow: u32,
}

impl Bus {
    fn new(data: [Output<'static>; 16], dc: Output<'static>, wr: Output<'static>) -> Self {
        // 引脚已在 esp-hal 里配成输出，这里读一次当前 out 值当影子
        let shadow = unsafe { (GPIO_OUT as *const u32).read_volatile() };
        Self {
            _data: data,
            _dc: dc,
            _wr: wr,
            shadow,
        }
    }

    /// 一次并口写：摆好数据与 DC，再打一个 WR 脉冲。
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
}

impl WriteOnlyDataCommand for Bus {
    fn send_commands(&mut self, cmd: DataFormat<'_>) -> Result<(), DisplayError> {
        let DataFormat::U8Iter(iter) = cmd else {
            return Err(DisplayError::DataFormatNotImplemented);
        };
        for byte in iter {
            self.write(u16::from(byte), false);
        }
        Ok(())
    }

    fn send_data(&mut self, buf: DataFormat<'_>) -> Result<(), DisplayError> {
        match buf {
            // 16 位并口下命令参数走 D[7:0]
            DataFormat::U8Iter(iter) => {
                for byte in iter {
                    self.write(u16::from(byte), true);
                }
            }
            // 像素一个写周期送完整 16 位 RGB565
            DataFormat::U16BEIter(iter) => {
                for pixel in iter {
                    self.write(pixel, true);
                }
            }
            _ => return Err(DisplayError::DataFormatNotImplemented),
        }
        Ok(())
    }
}

/// 库驱动 + 本仓的并口实现。
pub(crate) type Display = Ili9341<Bus, NoReset>;

/// 屏的初始化：硬件复位走空脚，随后由 `ili9341` 库发完整上电序列。
pub(crate) fn new(
    data: [Output<'static>; 16],
    dc: Output<'static>,
    wr: Output<'static>,
    delay: &mut Delay,
) -> Display {
    let display = Ili9341::new(
        Bus::new(data, dc, wr),
        NoReset,
        delay,
        Orientation::Landscape,
        DisplaySize240x320,
    )
    .expect("ili9341 init failed");
    info!("ili9341 ready {}x{}", display.width(), display.height());
    display
}
