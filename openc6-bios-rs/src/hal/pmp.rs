//! RISC-V PMP 硬件隔离窗口。
//!
//! 用 TOR（Top-Of-Range）模式占 4..7 号表项，把 U-Mode 限制在 sandbox arena 内：
//! 表项 8 之后一律 OFF，于是 MMIO（`0x60000000+`）默认拒绝。

use core::arch::asm;

/// PMP 表项权限位：可读。
const PMP_R: u8 = 1 << 0;
/// PMP 表项权限位：可写。
const PMP_W: u8 = 1 << 1;
/// PMP 表项权限位：可执行。
const PMP_X: u8 = 1 << 2;
/// PMP 地址匹配模式：TOR。
const PMP_A_TOR: u8 = 1 << 3;

/// flash IROM 起始地址，U-Mode 只读可执行。
const SOC_IROM_LOW: usize = 0x4200_0000;
/// flash IROM 结束地址。
const SOC_IROM_HIGH: usize = 0x4280_0000;

/// 配置 4..7 号表项把 U-Mode 圈在 `[arena_start, arena_end)`。
///
/// 传 `(0, 0)` 等于清空，回到全 M-Mode。
pub fn setup(arena_start: usize, arena_end: usize) {
    if arena_start == 0 && arena_end == 0 {
        unsafe {
            asm!(
                "csrw 0x3a0, zero",
                "csrw 0x3a1, zero",
                "csrw 0x3b4, zero",
                "csrw 0x3b5, zero",
                "csrw 0x3b6, zero",
                "csrw 0x3b7, zero",
                "fence.i",
                options(nostack)
            )
        };
        return;
    }

    // 表项 0..3 必须关掉，否则会和 arena 窗口抢地址匹配。
    unsafe {
        asm!("csrw 0x3a0, zero", "csrw 0x3b3, zero", options(nostack));
    }

    let addr4 = (arena_start >> 2) as u32;
    let addr5 = (arena_end >> 2) as u32;
    let addr6 = (SOC_IROM_LOW >> 2) as u32;
    let addr7 = (SOC_IROM_HIGH >> 2) as u32;

    // 表项 4：TOR NONE（内核 SRAM 下半）；表项 5：TOR RWX（arena）；
    // 表项 6：TOR NONE（内核 SRAM 上半）；表项 7：TOR R-X（flash）。
    let cfg = (PMP_A_TOR as u32)
        | ((PMP_A_TOR | PMP_R | PMP_W | PMP_X) as u32) << 8
        | ((PMP_A_TOR) as u32) << 16
        | ((PMP_A_TOR | PMP_R | PMP_X) as u32) << 24;

    unsafe {
        asm!(
            "csrw 0x3b4, {a4}",
            "csrw 0x3b5, {a5}",
            "csrw 0x3b6, {a6}",
            "csrw 0x3b7, {a7}",
            "csrw 0x3a1, {cfg}",
            "fence.i",
            a4 = in(reg) addr4,
            a5 = in(reg) addr5,
            a6 = in(reg) addr6,
            a7 = in(reg) addr7,
            cfg = in(reg) cfg,
            options(nostack)
        )
    };
}

/// 清空全部 sandbox PMP 表项。
pub fn clear() {
    setup(0, 0);
}
