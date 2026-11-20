//! 裸寄存器 MMIO 读写原语。

/// 读一个 32 位 MMIO 寄存器。
#[inline(always)]
pub fn read(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// 写一个 32 位 MMIO 寄存器。
#[inline(always)]
pub fn write(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

/// 对 32 位 MMIO 寄存器做读-改-写。
#[inline(always)]
pub fn modify(addr: usize, f: impl FnOnce(u32) -> u32) {
    write(addr, f(read(addr)));
}
