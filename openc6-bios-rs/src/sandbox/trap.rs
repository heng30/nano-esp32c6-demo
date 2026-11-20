//! U-Mode trap 向量、寄存器帧，以及 setjmp/longjmp。
//!
//! 向量表占 32 个 4 字节槽位，必须放在 IRAM（`.rwtext`）——文件系统擦写 flash 时
//! cache 会关闭，放在 flash 里的处理函数取不到指令。
//!
//! 中断槽全部转到保存下来的原始向量表，保留 esp-hal / esp-rtos 的既有中断处理；
//! 只有从 U-Mode 进来的中断才额外做一次特权级修补（见 `sandbox_intr_trampoline`）。

use core::arch::global_asm;

/// U-Mode trap 现场保存的寄存器帧，144 字节，与汇编里的栈布局逐字节对应。
#[repr(C)]
#[derive(Default)]
pub struct TrapFrame {
    /// `x1` 返回地址。
    pub ra: u32,
    /// `x2` 用户栈指针（trap 时从 `mscratch` 取回）。
    pub sp: u32,
    /// `x3` 全局指针。
    pub gp: u32,
    /// `x4` 线程指针。
    pub tp: u32,
    /// `x5` 临时寄存器 t0。
    pub t0: u32,
    /// `x6` 临时寄存器 t1。
    pub t1: u32,
    /// `x7` 临时寄存器 t2。
    pub t2: u32,
    /// `x8` 保存寄存器 s0 / 帧指针。
    pub s0: u32,
    /// `x9` 保存寄存器 s1。
    pub s1: u32,
    /// `x10` 参数寄存器 a0（系统调用返回值）。
    pub a0: u32,
    /// `x11` 参数寄存器 a1。
    pub a1: u32,
    /// `x12` 参数寄存器 a2。
    pub a2: u32,
    /// `x13` 参数寄存器 a3。
    pub a3: u32,
    /// `x14` 参数寄存器 a4。
    pub a4: u32,
    /// `x15` 参数寄存器 a5。
    pub a5: u32,
    /// `x16` 参数寄存器 a6。
    pub a6: u32,
    /// `x17` 参数寄存器 a7（系统调用号）。
    pub a7: u32,
    /// `x18` 保存寄存器 s2。
    pub s2: u32,
    /// `x19` 保存寄存器 s3。
    pub s3: u32,
    /// `x20` 保存寄存器 s4。
    pub s4: u32,
    /// `x21` 保存寄存器 s5。
    pub s5: u32,
    /// `x22` 保存寄存器 s6。
    pub s6: u32,
    /// `x23` 保存寄存器 s7。
    pub s7: u32,
    /// `x24` 保存寄存器 s8。
    pub s8: u32,
    /// `x25` 保存寄存器 s9。
    pub s9: u32,
    /// `x26` 保存寄存器 s10。
    pub s10: u32,
    /// `x27` 保存寄存器 s11。
    pub s11: u32,
    /// `x28` 临时寄存器 t3。
    pub t3: u32,
    /// `x29` 临时寄存器 t4。
    pub t4: u32,
    /// `x30` 临时寄存器 t5。
    pub t5: u32,
    /// `x31` 临时寄存器 t6。
    pub t6: u32,
    /// trap 发生时的 PC。
    pub mepc: u32,
    /// trap 原因。
    pub mcause: u32,
    /// trap 附加信息（出错的地址或指令）。
    pub mtval: u32,
}

/// 上下文保存缓冲，对应 `sandbox_setjmp` / `sandbox_longjmp`。
#[repr(C)]
#[derive(Default)]
pub struct JmpBuf {
    ra: u32,
    sp: u32,
    s0: u32,
    s1: u32,
    s2: u32,
    s3: u32,
    s4: u32,
    s5: u32,
    s6: u32,
    s7: u32,
    s8: u32,
    s9: u32,
    s10: u32,
    s11: u32,
}

unsafe extern "C" {
    /// 保存当前上下文，首次返回 0。
    pub fn sandbox_setjmp(buf: *mut JmpBuf) -> i32;
    /// 恢复到 `buf` 保存的上下文，表现为 `sandbox_setjmp` 返回 1。
    pub fn sandbox_longjmp(buf: *mut JmpBuf) -> !;
}

/// 进入沙箱前保存的原始 `mtvec`，含模式位，恢复时原样写回。
#[unsafe(no_mangle)]
pub static SANDBOX_SAVED_MTVEC: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// `mtvec` 掩掉低两位后的向量表基址，中断转发时按 `base + 4 * mcause` 寻址。
#[unsafe(no_mangle)]
pub static SANDBOX_VECTOR_BASE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

global_asm!(
    r#"
.section .rwtext, "ax"

/* ── 32 槽 trap 向量表，必须 256 字节对齐（硬件屏蔽低 8 位） ── */
.balign 0x100
.option push
.option norvc
.global sandbox_vector_table
sandbox_vector_table:
    j sandbox_trap_entry
    .rept 31
    j sandbox_intr_trampoline
    .endr
.option pop

/* ── 同步异常入口（槽 0） ── */
.balign 4
.option push
.option norvc
.global sandbox_trap_entry
sandbox_trap_entry:
    addi sp, sp, -16
    sw   t0, 0(sp)
    sw   t1, 4(sp)

    csrr t0, mstatus
    li   t1, (3 << 11)
    and  t1, t0, t1
    bnez t1, .Ltrap_forward

    /* 来自 U-Mode：换到内核 trap 栈，再压 144 字节现场 */
    lw   t0, 0(sp)
    lw   t1, 4(sp)
    addi sp, sp, 16

    csrrw sp, 0x340, sp
    addi sp, sp, -144

    sw x1,   0(sp)
    csrr x1, 0x340
    sw x1,   4(sp)

    sw x3,   8(sp)
    sw x4,  12(sp)
    sw x5,  16(sp)
    sw x6,  20(sp)
    sw x7,  24(sp)
    sw x8,  28(sp)
    sw x9,  32(sp)
    sw x10, 36(sp)
    sw x11, 40(sp)
    sw x12, 44(sp)
    sw x13, 48(sp)
    sw x14, 52(sp)
    sw x15, 56(sp)
    sw x16, 60(sp)
    sw x17, 64(sp)
    sw x18, 68(sp)
    sw x19, 72(sp)
    sw x20, 76(sp)
    sw x21, 80(sp)
    sw x22, 84(sp)
    sw x23, 88(sp)
    sw x24, 92(sp)
    sw x25, 96(sp)
    sw x26, 100(sp)
    sw x27, 104(sp)
    sw x28, 108(sp)
    sw x29, 112(sp)
    sw x30, 116(sp)
    sw x31, 120(sp)

    csrr t0, 0x341
    csrr t1, 0x342
    csrr t2, 0x343
    sw t0, 124(sp)
    sw t1, 128(sp)
    sw t2, 132(sp)

    mv   a0, sp
    mv   a1, t1
    mv   a2, t2
    call sandbox_dispatch_trap

    /* 处理函数可能改过 mepc（例如跳过 ecall） */
    lw t0, 124(sp)
    csrw 0x341, t0

    lw x1,   0(sp)
    lw x3,   8(sp)
    lw x4,  12(sp)
    lw x5,  16(sp)
    lw x6,  20(sp)
    lw x7,  24(sp)
    lw x8,  28(sp)
    lw x9,  32(sp)
    lw x10, 36(sp)
    lw x11, 40(sp)
    lw x12, 44(sp)
    lw x13, 48(sp)
    lw x14, 52(sp)
    lw x15, 56(sp)
    lw x16, 60(sp)
    lw x17, 64(sp)
    lw x18, 68(sp)
    lw x19, 72(sp)
    lw x20, 76(sp)
    lw x21, 80(sp)
    lw x22, 84(sp)
    lw x23, 88(sp)
    lw x24, 92(sp)
    lw x25, 96(sp)
    lw x26, 100(sp)
    lw x27, 104(sp)
    lw x28, 108(sp)
    lw x29, 112(sp)
    lw x30, 116(sp)
    lw x31, 120(sp)

    lw   t0, 4(sp)
    csrw 0x340, t0
    addi sp, sp, 144
    csrrw sp, 0x340, sp
    mret

.Ltrap_forward:
    /* 来自 M-Mode 的异常：交回 esp-hal 原来的异常入口 */
    lw   t0, 0(sp)
    lw   t1, 4(sp)
    addi sp, sp, 16
    la   t2, SANDBOX_VECTOR_BASE
    lw   t2, 0(t2)
    jr   t2
.option pop

/* ── 异步中断入口（槽 1..31） ── */
.balign 4
.option push
.option norvc
.global sandbox_intr_trampoline
sandbox_intr_trampoline:
    addi sp, sp, -16
    sw   t0, 0(sp)
    sw   t1, 4(sp)

    csrr t0, mstatus
    li   t1, (3 << 11)
    and  t1, t0, t1
    bnez t1, .Lintr_from_mmode

    /* 来自 U-Mode：记下原 PC 与本任务的 trap 栈，把 mepc 改到返回跳板，
       MPP 抬到 3，避免调度器切到别的内核任务时被 mret 掉进 U-Mode。 */
    csrr t0, mepc
    sw   t0, 8(sp)
    csrr t0, 0x340
    sw   t0, 12(sp)

    la   t0, sandbox_umode_return
    csrw mepc, t0

    csrr t0, mstatus
    li   t1, (3 << 11)
    or   t0, t0, t1
    csrw mstatus, t0

    lw   t0, 0(sp)
    lw   t1, 4(sp)
    j    .Lintr_dispatch

.Lintr_from_mmode:
    lw   t0, 0(sp)
    lw   t1, 4(sp)
    addi sp, sp, 16

.Lintr_dispatch:
    la   t2, SANDBOX_VECTOR_BASE
    lw   t2, 0(t2)
    csrr t3, mcause
    slli t3, t3, 2
    add  t2, t2, t3
    jr   t2
.option pop

/* ── U-Mode 中断返回跳板 ── */
.balign 4
.option push
.option norvc
.global sandbox_umode_return
sandbox_umode_return:
    csrci mstatus, 8

    sw   t0, 0(sp)
    sw   t1, 4(sp)

    lw   t0, 12(sp)
    csrw 0x340, t0

    lw   t0, 8(sp)
    csrw 0x341, t0

    csrr t0, mstatus
    li   t1, 0xFFFFE7FF
    and  t0, t0, t1
    ori  t0, t0, (1 << 7)
    csrw mstatus, t0

    lw   t0, 0(sp)
    lw   t1, 4(sp)
    addi sp, sp, 16
    mret
.option pop

/* ── setjmp / longjmp ── */
.balign 4
.option push
.option norvc
.global sandbox_setjmp
sandbox_setjmp:
    sw ra,  0(a0)
    sw sp,  4(a0)
    sw s0,  8(a0)
    sw s1, 12(a0)
    sw s2, 16(a0)
    sw s3, 20(a0)
    sw s4, 24(a0)
    sw s5, 28(a0)
    sw s6, 32(a0)
    sw s7, 36(a0)
    sw s8, 40(a0)
    sw s9, 44(a0)
    sw s10, 48(a0)
    sw s11, 52(a0)
    li a0, 0
    ret

.global sandbox_longjmp
sandbox_longjmp:
    lw ra,  0(a0)
    lw sp,  4(a0)
    lw s0,  8(a0)
    lw s1, 12(a0)
    lw s2, 16(a0)
    lw s3, 20(a0)
    lw s4, 24(a0)
    lw s5, 28(a0)
    lw s6, 32(a0)
    lw s7, 36(a0)
    lw s8, 40(a0)
    lw s9, 44(a0)
    lw s10, 48(a0)
    lw s11, 52(a0)
    li a0, 1
    ret
.option pop
"#
);
