//! 裸金属 GPIO 驱动（ESP32-C6 HP GPIO + IO_MUX）。
//!
//! 用 W1TS / W1TC 原子寄存器置位清零，不关中断也没有竞态。

use super::regs::{modify, read, write};

/// HP GPIO 外设基地址。
const GPIO_BASE: usize = 0x6009_1000;
/// 输出置 1（write-1-to-set）。
const GPIO_OUT_W1TS: usize = GPIO_BASE + 0x0008;
/// 输出清 0（write-1-to-clear）。
const GPIO_OUT_W1TC: usize = GPIO_BASE + 0x000C;
/// 输出使能置 1。
const GPIO_ENABLE_W1TS: usize = GPIO_BASE + 0x0024;
/// 输出使能清 0。
const GPIO_ENABLE_W1TC: usize = GPIO_BASE + 0x0028;
/// 输入电平。
const GPIO_IN: usize = GPIO_BASE + 0x003C;

/// IO_MUX 基地址。
const IO_MUX_BASE: usize = 0x6009_0000;

/// IO_MUX：内部下拉使能（bit 7）。
const FUN_WPD: u32 = 1 << 7;
/// IO_MUX：内部上拉使能（bit 8）。
const FUN_WPU: u32 = 1 << 8;
/// IO_MUX：输入使能（bit 9）。
const FUN_IE: u32 = 1 << 9;
/// IO_MUX：MCU_SEL 字段掩码（bit 12..14）。
const MCU_SEL_MASK: u32 = 0x7 << 12;
/// IO_MUX：MCU_SEL = 1 即直连 GPIO 矩阵。
const MCU_SEL_GPIO: u32 = 1 << 12;

/// 电源键的虚拟地锁存线。
pub const PIN_PWR_BTN_GND: u32 = 3;
/// 电源键 sense 线。
pub const PIN_PWR_BTN_SENSE: u32 = 4;
/// 板载 WS2812 数据线。
/// 板载 WS2812 数据线。
pub const PIN_WS2812: u32 = 8;
/// 物理 BOOT 键。
pub const PIN_BOOT_BTN: u32 = 9;
/// Clear CMOS 跳线的接地端。
pub const PIN_CMOS_GND: u32 = 1;
/// Clear CMOS 跳线的检测端。
pub const PIN_CLEAR_NVRAM: u32 = 2;

/// 内核锁定、不向 U-Mode 开放的引脚位掩码。
pub const PROTECTED_MASK: u32 = (1 << PIN_CMOS_GND)
    | (1 << PIN_CLEAR_NVRAM)
    | (1 << PIN_PWR_BTN_GND)
    | (1 << PIN_PWR_BTN_SENSE)
    | (1 << PIN_WS2812)
    | (1 << PIN_BOOT_BTN)
    | (1 << 12)
    | (1 << 13)
    | (0x7F << 24);

/// 引脚能否被 U-Mode 访问。
#[inline]
pub fn is_safe(pin: u32) -> bool {
    pin <= 30 && PROTECTED_MASK & (1 << pin) == 0
}

/// 某个引脚的 IO_MUX 配置寄存器地址。
#[inline]
fn iomux_reg(pin: u32) -> usize {
    IO_MUX_BASE + 0x04 + (pin as usize) * 4
}

/// 把引脚切到 GPIO 功能，并可选上/下拉。
fn route_to_gpio(pin: u32, pull_up: bool, pull_down: bool, input: bool) {
    let reg = iomux_reg(pin);
    modify(reg, |v| {
        let mut v = (v & !MCU_SEL_MASK) | MCU_SEL_GPIO;
        v &= !(FUN_WPU | FUN_WPD);
        if pull_up {
            v |= FUN_WPU;
        }
        if pull_down {
            v |= FUN_WPD;
        }
        if input {
            v |= FUN_IE;
        } else {
            v &= !FUN_IE;
        }
        v
    });
}

/// 把引脚配置成 GPIO 输出。
pub fn set_output(pin: u32) {
    route_to_gpio(pin, false, false, false);
    write(GPIO_ENABLE_W1TS, 1 << pin);
}

/// 把引脚配置成 GPIO 输入，`pull_up` 决定是否开内部上拉。
pub fn set_input(pin: u32, pull_up: bool) {
    route_to_gpio(pin, pull_up, false, true);
    write(GPIO_ENABLE_W1TC, 1 << pin);
}

/// 断开引脚的 GPIO 功能（恢复高阻）。
pub fn set_disabled(pin: u32) {
    write(GPIO_ENABLE_W1TC, 1 << pin);
    modify(iomux_reg(pin), |v| v & !(FUN_IE | FUN_WPU | FUN_WPD));
}

/// 设置输出电平。
pub fn write_pin(pin: u32, level: bool) {
    if level {
        write(GPIO_OUT_W1TS, 1 << pin);
    } else {
        write(GPIO_OUT_W1TC, 1 << pin);
    }
}

/// 读输入电平。
pub fn read_pin(pin: u32) -> bool {
    read(GPIO_IN) & (1 << pin) != 0
}

/// U-Mode 视角的 `gpio_set_dir`：受保护引脚返回 `-1`。
pub fn user_set_dir(pin: u32, is_output: bool) -> i32 {
    if !is_safe(pin) {
        return -1;
    }
    if is_output {
        set_output(pin);
    } else {
        set_input(pin, false);
    }
    0
}

/// U-Mode 视角的 `gpio_write`：受保护引脚返回 `-1`。
pub fn user_write(pin: u32, level: bool) -> i32 {
    if !is_safe(pin) {
        return -1;
    }
    write_pin(pin, level);
    0
}

/// U-Mode 视角的 `gpio_read`：受保护引脚返回 `-1`。
pub fn user_read(pin: u32) -> i32 {
    if !is_safe(pin) {
        return -1;
    }
    read_pin(pin) as i32
}
