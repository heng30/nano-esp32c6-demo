//! RISC-V 机器模式 CSR 操作与 U-Mode 特权切换。

use core::arch::asm;

/// `mstatus` 的 MPIE 位（mret 后恢复的中断使能）。
const MSTATUS_MPIE: u32 = 1 << 7;
/// `mstatus` 的 MPP 字段（mret 目标特权级，3 = M-Mode，0 = U-Mode）。
const MSTATUS_MPP: u32 = 3 << 11;

/// 读 `mie`。
#[inline(always)]
pub fn read_mie() -> u32 {
    let v: u32;
    unsafe { asm!("csrr {}, mie", out(reg) v, options(nomem, nostack)) };
    v
}

/// 写 `mie`。
#[inline(always)]
pub fn write_mie(v: u32) {
    unsafe { asm!("csrw mie, {}", in(reg) v, options(nomem, nostack)) };
}

/// 读 `mscratch`。
#[inline(always)]
pub fn read_mscratch() -> u32 {
    let v: u32;
    unsafe { asm!("csrr {}, mscratch", out(reg) v, options(nomem, nostack)) };
    v
}

/// 写 `mscratch`。
#[inline(always)]
pub fn write_mscratch(v: u32) {
    unsafe { asm!("csrw mscratch, {}", in(reg) v, options(nomem, nostack)) };
}

/// 写 `mtvec`。
#[inline(always)]
pub fn write_mtvec(v: u32) {
    unsafe { asm!("csrw mtvec, {}", in(reg) v, options(nomem, nostack)) };
}

/// 把 `mtvec` 换成向量表地址（低位置 1 表示向量模式），返回旧值。
#[inline(always)]
pub fn swap_mtvec_vectored(table_addr: usize) -> u32 {
    let old: u32;
    let table = ((table_addr as u32) & !0xFF) | 0x01;
    unsafe { asm!("csrrw {}, mtvec, {}", out(reg) old, in(reg) table, options(nomem, nostack)) };
    old
}

/// 从链接脚本重新装载全局指针 `gp`。
///
/// shortcut: `gp` 是 LLVM 保留寄存器，不能声明为 asm 操作数，改动它编译器不会知道；
/// 只在刚从 U-Mode trap 回来、准备恢复内核执行环境时调用。
#[inline(always)]
pub fn restore_gp() {
    unsafe {
        asm!(
            ".option push",
            ".option norelax",
            "la gp, __global_pointer$",
            ".option pop",
            options(nomem, nostack)
        )
    };
}

/// 开中断。
///
/// trap 进入时硬件会清 `mstatus.MIE`；如果从 trap 里 `longjmp` 出去而没有 `mret`，
/// 中断会一直关着，RTOS tick 停摆、执行器冻死。
#[inline(always)]
pub fn irq_enable() {
    unsafe { asm!("csrs mstatus, 8", options(nomem, nostack)) };
}

/// 强制 `mstatus.MPP = 3`，让下一次 `mret` 回到 M-Mode。
#[inline(always)]
pub fn force_mmode_mpp() {
    let mut v: u32;
    unsafe {
        asm!("csrr {}, mstatus", out(reg) v, options(nomem, nostack));
        v |= MSTATUS_MPP;
        asm!("csrw mstatus, {}", in(reg) v, options(nomem, nostack));
    }
}

/// 关中断进入 U-Mode：`mscratch` 装内核 trap 栈，`sp` 换用户栈，`a0` 传参数。
///
/// # Safety
/// `entry` / `user_sp` 必须指向可执行 / 可写的合法内存，且调用后不再返回。
#[inline(always)]
pub unsafe fn enter_umode(entry: usize, user_sp: usize, arg0: usize, kernel_trap_sp: usize) -> ! {
    unsafe {
        asm!("csrci mstatus, 8", options(nomem, nostack));

        let mut v: u32;
        asm!("csrr {}, mstatus", out(reg) v, options(nomem, nostack));
        v &= !MSTATUS_MPP;
        v |= MSTATUS_MPIE;
        asm!("csrw mstatus, {}", in(reg) v, options(nomem, nostack));

        asm!(
            "csrw 0x340, {kernel}",
            "csrw 0x341, {entry}",
            "mv sp, {sp}",
            "mv a0, {arg0}",
            "fence.i",
            "mret",
            kernel = in(reg) kernel_trap_sp,
            entry = in(reg) entry,
            sp = in(reg) user_sp,
            arg0 = in(reg) arg0,
            options(noreturn)
        )
    }
}
