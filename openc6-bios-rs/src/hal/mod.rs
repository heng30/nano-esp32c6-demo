//! OpenC6 裸金属硬件抽象层。
//!
//! 全部直接操作 ESP32-C6 寄存器，不依赖 esp-hal 的外设驱动——BIOS 需要在中断屏蔽、
//! trap 上下文和 U-Mode 边界上都能工作。

pub mod clock;
pub mod console;
pub mod cpu;
pub mod flash;
pub mod gpio;
pub mod pmp;
pub mod power;
pub mod regs;
pub mod wdt;
