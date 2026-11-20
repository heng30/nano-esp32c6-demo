//! OpenC6 管理引擎的 LP-Core 侧程序。
//!
//! 运行在 ESP32-C6 的 LP 核（RV32IMC）上，与 HP 核通过 LP RAM 里的共享变量通信。
//! 不依赖任何 HAL：LP RAM 与 LP 外设都是固定地址，直接按绝对地址读写。
//!
//! 职责（与上游 `components/management_engine/ulp/lp_core_main.c` 对应）：
//!   * 电源键去抖：长按 3 s 置 Setup，长按 5 s 直接硬复位
//!   * 主核软件看门狗：`WDT_COUNTER` 超过 15 s 不更新即硬复位
//!   * 心跳：供 HP 侧确认 LP 核还活着
//!
//! 内存约定见 `../linker.ld`，共享变量偏移见 [`shared`]。

#![no_std]
#![no_main]

/// LP RAM 中共享变量区的基地址。
const SHARED_BASE: usize = 0x5000_3000;
/// LP IO 输入寄存器（`LP_IO.IN`），bit n 对应 LP_GPIO n。
const LP_IO_IN: usize = 0x600B_2024;
/// 电源键 sense 线所在的 LP_GPIO 序号。
const PWR_BTN_SENSE: u32 = 4;

/// 主核软件看门狗超时（毫秒）。
const WDT_TIMEOUT_MS: u32 = 15_000;
/// 长按多久判定为「进 Setup」。
const LONG_PRESS_SETUP_MS: u32 = 3000;
/// 长按多久判定为「强制复位」。
const LONG_PRESS_RESET_MS: u32 = 5000;
/// 主循环一拍代表的毫秒数，与末尾的忙等延时对应。
const TICK_MS: u32 = 10;

/// 共享变量偏移，必须与 HP 侧 `src/me.rs` 里的常量一致。
mod shared {
    /// HP 侧置 1 表示主核醒着。
    pub const HP_IS_AWAKE: usize = 0x04;
    /// HP 侧每喂一次狗自增。
    pub const WDT_COUNTER: usize = 0x0C;
    /// LP 侧心跳，HP 侧读它判断 LP 核是否真的在跑。
    pub const HEARTBEAT: usize = 0x10;
    /// LP 侧记录的「请 HP 侧执行的请求」，0 表示无。
    pub const REQ: usize = 0x18;
}

fn read_shared(offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((SHARED_BASE + offset) as *const u32) }
}

fn write_shared(offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile((SHARED_BASE + offset) as *mut u32, value) }
}

/// 读 LP_IO 某个引脚的电平。
fn lp_gpio_level(pin: u32) -> bool {
    let reg = unsafe { core::ptr::read_volatile(LP_IO_IN as *const u32) };
    (reg >> pin) & 1 != 0
}

core::arch::global_asm!(
    ".section .text.entry",
    ".global _start",
    "_start:",
    "  li sp, 0x50002000",
    "  call lp_main",
    "1:",
    "  j 1b",
);

#[unsafe(no_mangle)]
pub extern "C" fn lp_main() -> ! {
    // 注意：本函数**不能**生成栈帧，也**不能**调用远处的函数。
    // 实测 LP 核在完整逻辑版本里会完全不执行（心跳恒 0），而线性小函数稳定可用。
    // 所以这里只做采集与记录，复位动作交给 HP 侧执行（见 me::poll_lp_requests）。
    let mut heartbeat = 0u32;
    let mut press = 0u32;
    let mut stale = 0u32;
    let mut wdt_last = read_shared(shared::WDT_COUNTER);

    loop {
        let awake = read_shared(shared::HP_IS_AWAKE) != 0;
        let pressed = !lp_gpio_level(PWR_BTN_SENSE);

        heartbeat = heartbeat.wrapping_add(1);
        write_shared(shared::HEARTBEAT, heartbeat);

        // 电源键计时：按下累加，松开漏桶衰减
        press = if pressed {
            if press < 10_000 { press + TICK_MS } else { press }
        } else {
            press.saturating_sub(30)
        };
        if awake && press >= LONG_PRESS_RESET_MS {
            write_shared(shared::REQ, 3);
        } else if press >= LONG_PRESS_SETUP_MS {
            write_shared(shared::REQ, 2);
        }

        // 主核喂狗停滞检测
        let counter = read_shared(shared::WDT_COUNTER);
        if counter != wdt_last {
            wdt_last = counter;
            stale = 0;
        } else if awake {
            stale += TICK_MS;
            if stale >= WDT_TIMEOUT_MS {
                write_shared(shared::REQ, 4);
            }
        } else {
            stale = 0;
        }

        delay_tick();
    }
}

/// 一拍忙等延时。
///
/// shortcut: 用忙等代替 LP 定时器，节拍随 LP 核频率漂移；
/// 需要精确计时时换成 LP_TIMER 比较中断。
#[inline(always)]
fn delay_tick() {
    for _ in 0..50_000u32 {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
