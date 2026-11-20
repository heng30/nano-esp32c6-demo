//! 硬件看门狗控制：TIMG0/TIMG1 主看门狗，以及 LP_WDT 的系统复位触发。

use core::sync::atomic::{AtomicU32, Ordering};

use super::regs::{read, write};

/// TIMG0 看门狗配置寄存器 0。
const TIMG0_WDTCONFIG0: usize = 0x6000_8048;
/// TIMG0 看门狗喂狗寄存器。
const TIMG0_WDTFEED: usize = 0x6000_8060;
/// TIMG0 看门狗写保护寄存器。
const TIMG0_WDTWPROTECT: usize = 0x6000_8064;
/// TIMG0 中断清除寄存器。
const TIMG0_INT_CLR: usize = 0x6000_8078;

/// TIMG1 看门狗配置寄存器 0。
const TIMG1_WDTCONFIG0: usize = 0x6000_9048;
/// TIMG1 看门狗喂狗寄存器。
const TIMG1_WDTFEED: usize = 0x6000_9060;
/// TIMG1 看门狗写保护寄存器。
const TIMG1_WDTWPROTECT: usize = 0x6000_9064;
/// TIMG1 中断清除寄存器。
const TIMG1_INT_CLR: usize = 0x6000_9078;

/// TIMG 写保护解锁魔数。
const TIMG_WDT_KEY: u32 = 0x50D8_3AA1;
/// TIMG 中断清除位（bit 1）。
const TIMG_WDT_INT_CLR_BIT: u32 = 1 << 1;

/// LP_WDT 基地址。
/// LP_WDT 配置寄存器 0。
/// LP_WDT 配置寄存器 1（超时周期）。
/// LP_WDT 喂狗寄存器。
/// LP_WDT 写保护寄存器。
/// LP_WDT 写保护解锁魔数。
/// LP_WDT：Stage 0 动作字段的位偏移。
/// LP_WDT：Stage 0 动作字段掩码。
/// LP_WDT：Stage 0 动作 4 = 系统 + RTC 复位。
/// LP_WDT：允许复位整颗芯片。
/// LP_WDT：允许复位 CPU0。
/// LP_WDT：复位后仍保持看门狗保护。
/// LP_WDT：使能位。
/// 保存下来的 TIMG0 看门狗配置。
static SAVED_TIMG0: AtomicU32 = AtomicU32::new(0);
/// 保存下来的 TIMG1 看门狗配置。
static SAVED_TIMG1: AtomicU32 = AtomicU32::new(0);

fn timg_write(protect: usize, cfg: usize, feed: usize, val: u32) {
    write(protect, TIMG_WDT_KEY);
    write(cfg, val);
    write(feed, 1);
    write(protect, 0);
}

/// 保存并关闭两路 TIMG 看门狗，进 U-Mode 前调用。
pub fn timg_save_and_disable() {
    SAVED_TIMG0.store(read(TIMG0_WDTCONFIG0), Ordering::Relaxed);
    timg_write(TIMG0_WDTWPROTECT, TIMG0_WDTCONFIG0, TIMG0_WDTFEED, 0);

    SAVED_TIMG1.store(read(TIMG1_WDTCONFIG0), Ordering::Relaxed);
    timg_write(TIMG1_WDTWPROTECT, TIMG1_WDTCONFIG0, TIMG1_WDTFEED, 0);
}

/// 恢复进 U-Mode 前的 TIMG 看门狗配置。
pub fn timg_restore() {
    timg_write(
        TIMG0_WDTWPROTECT,
        TIMG0_WDTCONFIG0,
        TIMG0_WDTFEED,
        SAVED_TIMG0.load(Ordering::Relaxed),
    );
    timg_write(
        TIMG1_WDTWPROTECT,
        TIMG1_WDTCONFIG0,
        TIMG1_WDTFEED,
        SAVED_TIMG1.load(Ordering::Relaxed),
    );
}

/// 清中断并彻底停掉两路 TIMG 看门狗。
pub fn timg_clear_and_halt() {
    write(TIMG0_WDTWPROTECT, TIMG_WDT_KEY);
    write(TIMG0_INT_CLR, TIMG_WDT_INT_CLR_BIT);
    timg_write(TIMG0_WDTWPROTECT, TIMG0_WDTCONFIG0, TIMG0_WDTFEED, 0);

    write(TIMG1_WDTWPROTECT, TIMG_WDT_KEY);
    write(TIMG1_INT_CLR, TIMG_WDT_INT_CLR_BIT);
    timg_write(TIMG1_WDTWPROTECT, TIMG1_WDTCONFIG0, TIMG1_WDTFEED, 0);
}

