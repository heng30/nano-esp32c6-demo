//! 管理引擎（Management Engine）的 HP-Core 侧。
//!
//! 上游把看门狗、温控和调频全放在 LP-Core（ULP RISC-V 协处理器）上跑，
//! 通过共享的 LP-RAM 变量与主核通信。
//!
//! LP 程序在 `ulp/` 下独立编译（目标、链接脚本、栈都不同），产物
//! `ulp/me-core.bin` 在编译期被嵌进固件，运行时拷到 LP RAM 起始处再由
//! [`start_lp_core`] 启动。共享变量区的布局见 [`shared`] 与 `ulp/src/main.rs`。
//!
//! LP 核目前只跑心跳；上游那套电源键去抖 / 自治调频 / 过热复位会逐步补上。

use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use critical_section::Mutex;

use crate::{hal::wdt, nvram};

/// LP 程序镜像，由 `ulp/` 单独编译后 objcopy 成扁平二进制。
const LP_CORE_IMAGE: &[u8] = include_bytes!("../ulp/me-core.bin");

/// LP RAM 起始地址，LP 核复位后从这里取指。
const LP_RAM_BASE: usize = 0x5000_0000;

/// HP / LP 共享变量区在 LP RAM 内的字节偏移。
const SHARED_OFFSET: usize = 0x3000;

/// LP IO 外设基址。
const LP_IO_BASE: usize = 0x600B_2000;
/// LP IO 里 `GPIO[n]` 配置寄存器的起始偏移。
const LP_IO_GPIO_OFFSET: usize = 0x48;
/// 输出清零寄存器偏移。
const LP_IO_OUT_DATA_W1TC: usize = 0x08;
/// 输出使能置位寄存器偏移。
const LP_IO_OUT_ENABLE_W1TS: usize = 0x10;
/// `GPIO[n].fun_wpu` 位，开内部上拉。
const LP_IO_FUN_WPU: u32 = 1 << 8;
/// `GPIO[n].fun_ie` 位，开输入通路。
const LP_IO_FUN_IE: u32 = 1 << 9;
/// `GPIO[n].mcu_sel` 掩码，0 表示走 LP_GPIO 功能。
const LP_IO_MCU_SEL_MASK: u32 = 0x7 << 12;

/// 电源键虚拟地引脚（LP_GPIO3）。
const PIN_PWR_GND: u32 = 3;
/// 电源键 sense 引脚（LP_GPIO4）。
const PIN_PWR_SENSE: u32 = 4;

/// 配置电源键的两个引脚。
///
/// 必须写 **LP_IO** 的 `GPIO[n]`，不能只写 HP 侧 IO_MUX：LP 核读的是 `LP_IO.IN`，
/// HP 侧配好上拉但 LP 侧输入通路没开时，LP 核会一直读到 0，把按钮误判成常按。
pub fn config_power_button() {
    let base = LP_IO_BASE;

    // GPIO3 输出低：给按键提供虚拟地
    unsafe {
        core::ptr::write_volatile((base + LP_IO_OUT_DATA_W1TC) as *mut u32, 1 << PIN_PWR_GND);
        core::ptr::write_volatile((base + LP_IO_OUT_ENABLE_W1TS) as *mut u32, 1 << PIN_PWR_GND);
    }

    // GPIO4 输入 + 上拉
    let reg = base + LP_IO_GPIO_OFFSET + 4 * PIN_PWR_SENSE as usize;
    unsafe {
        let mut v = core::ptr::read_volatile(reg as *const u32);
        v = (v & !LP_IO_MCU_SEL_MASK) | LP_IO_FUN_IE | LP_IO_FUN_WPU;
        core::ptr::write_volatile(reg as *mut u32, v);
    }
}

/// 人为抑制喂狗的截止时刻（embassy 毫秒），0 表示不抑制。
///
/// 仅用于验证 LP 看门狗：让 shell 照常跑、执行器保持健康，
/// 唯独不喂狗，这样复位只可能来自 LP 核。
static PET_SUPPRESS_UNTIL: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// 处理 LP 核记录的请求。
///
/// LP 核只负责采集与记录（它的执行约束见 `ulp/src/main.rs` 顶部注释），
/// 真正的动作在 HP 侧做。必须在 shell 空闲循环里周期调用。
pub fn poll_lp_requests() {
    if !is_active() {
        return;
    }

    let req = shared_read(shared::REQ);
    if req == 0 {
        return;
    }

    match req {
        2 => crate::hal::console::print("\r\n[LP] 电源键长按 3 秒：请运行 setup 进入 BIOS Setup\r\n"),
        // 复位前留时间把日志送出 USB CDC，否则消息会随复位丢失
        3 => {
            crate::hal::console::print("\r\n[LP] 电源键长按 5 秒，强制复位\r\n");
            flush_console();
            crate::hal::power::reboot();
        }
        4 => {
            crate::hal::console::print("\r\n[LP] 主核喂狗停滞超时，执行复位\r\n");
            flush_console();
            crate::hal::power::reboot();
        }
        _ => {}
    }

    // 请求已消费，清掉避免重复触发
    shared_write(shared::REQ, 0);
}

/// 阻塞等待，把控制台缓冲刷出去。
fn flush_console() {
    let deadline = esp_hal::time::Instant::now() + esp_hal::time::Duration::from_millis(300);
    while esp_hal::time::Instant::now() < deadline {
        core::hint::spin_loop();
    }
}

/// 在接下来 `secs` 秒内停止喂狗。
pub fn suppress_petting(secs: u64) {
    use core::sync::atomic::Ordering;
    PET_SUPPRESS_UNTIL.store(
        (embassy_time::Instant::now().as_millis() + secs * 1000) as u32,
        Ordering::Relaxed,
    );
}

/// 当前是否处于人为抑制窗口内。
fn petting_suppressed() -> bool {
    use core::sync::atomic::Ordering;
    let until = PET_SUPPRESS_UNTIL.load(Ordering::Relaxed);
    until != 0 && (embassy_time::Instant::now().as_millis() as u32) < until
}

/// 共享变量偏移，必须与 `ulp/src/main.rs` 的 `shared` 模块逐项一致。
mod shared {
    /// HP 侧写 0 可让 LP 程序空转。
    pub const IS_ENABLED: usize = 0x00;
    /// HP 侧置 1 表示主核醒着。
    pub const HP_IS_AWAKE: usize = 0x04;
    /// LP 侧写入的重启原因。
    pub const BOOT_REASON: usize = 0x08;
    /// HP 侧每喂一次狗自增。
    pub const WDT_COUNTER: usize = 0x0C;
    /// LP 侧心跳，HP 侧读它判断 LP 核是否真的在跑。
    pub const HEARTBEAT: usize = 0x10;
    /// LP 侧记录的「请 HP 侧执行的请求」：0 无、2 长按进 Setup、3 长按强复位、4 喂狗停滞。
    pub const REQ: usize = 0x18;
    /// HP 侧忙计数。
    pub const HP_BUSY_TICKS: usize = 0x1C;
    /// HP 侧闲计数。
    pub const HP_IDLE_TICKS: usize = 0x20;
    /// LP 侧算出的 CPU 占用百分比。
    pub const CPU_LOAD: usize = 0x24;
    /// LP 侧实际下发的频率（MHz）。
    pub const TARGET_FREQ: usize = 0x28;
}

/// 把 NVRAM 的频率档位换算成 MHz。
fn cpu_freq_mhz(f: nvram::CpuFreq) -> u32 {
    match f {
        nvram::CpuFreq::Mhz80 => 80,
        nvram::CpuFreq::Mhz120 => 120,
        nvram::CpuFreq::Mhz160 => 160,
    }
}

/// LP 侧算出的 CPU 占用百分比。
pub fn lp_cpu_load() -> u32 {
    shared_read(shared::CPU_LOAD)
}

/// LP 侧实际下发的频率（MHz）。
pub fn lp_target_freq() -> u32 {
    shared_read(shared::TARGET_FREQ)
}

/// 记一次「忙」：Shell 正在执行命令。
pub fn note_busy() {
    if is_active() {
        let v = shared_read(shared::HP_BUSY_TICKS).wrapping_add(1);
        shared_write(shared::HP_BUSY_TICKS, v);
    }
}

/// 记一次「闲」：Shell 在等输入。
pub fn note_idle() {
    if is_active() {
        let v = shared_read(shared::HP_IDLE_TICKS).wrapping_add(1);
        shared_write(shared::HP_IDLE_TICKS, v);
    }
}

/// 共享区某个变量字地址。
fn shared_word(offset: usize) -> *mut u32 {
    (LP_RAM_BASE + SHARED_OFFSET + offset) as *mut u32
}

/// 读共享变量。
fn shared_read(offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile(shared_word(offset)) }
}

/// 写共享变量。
fn shared_write(offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile(shared_word(offset), value) }
}

/// 启动 LP 核并把管理引擎镜像装载进去。
///
/// 返回 `false` 表示 LP 核没能跑起来（此时 HP 侧仍可独立工作）。
pub fn start_lp_core(lp_core: esp_hal::peripherals::LP_CORE<'static>) -> bool {
    let mut core = esp_hal::lp_core::LpCore::new(lp_core);

    // `LpCore::new` 会把整个 16 KB LP RAM 清零，镜像必须在它之后写
    let ram = unsafe {
        core::slice::from_raw_parts_mut(LP_RAM_BASE as *mut u8, LP_CORE_IMAGE.len())
    };
    ram.copy_from_slice(LP_CORE_IMAGE);

    // 共享区清零后放行 LP 程序
    for offset in [shared::IS_ENABLED, shared::HP_IS_AWAKE, shared::BOOT_REASON, shared::WDT_COUNTER, shared::HEARTBEAT] {
        shared_write(offset, 0);
    }
    shared_write(shared::IS_ENABLED, 1);
    shared_write(shared::HP_IS_AWAKE, 1);

    core.run(esp_hal::lp_core::LpCoreWakeupSource::HpCpu);
    defmt::info!("[ME] LP-Core 镜像已装载 ({} B)", LP_CORE_IMAGE.len());
    true
}

/// LP 核当前心跳计数。
///
/// 两次读取之间数值不变，说明 LP 核没跑起来或已停摆。
pub fn lp_heartbeat() -> u32 {
    shared_read(shared::HEARTBEAT)
}

/// 系统启动原因，数值与上游 `ME_BOOT_REASON_*` 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum BootReason {
    /// 无特殊原因。
    None,
    /// 正常上电。
    Normal,
    /// 电源键长按进入 Setup。
    Setup,
    /// 电源键长按强制复位。
    ForceReset,
    /// 看门狗复位。
    Watchdog,
    /// 过热紧急复位。
    Thermal,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static BOOT_REASON: AtomicU32 = AtomicU32::new(BootReason::None as u32);
static TEMPERATURE: AtomicU32 = AtomicU32::new(0);
static SENSOR: Mutex<RefCell<Option<esp_hal::tsens::TemperatureSensor<'static>>>> =
    Mutex::new(RefCell::new(None));

/// 启动管理引擎，接管看门狗与温度采样。
pub fn init(enabled: bool, is_cold_boot: bool) {
    ACTIVE.store(enabled, Ordering::Release);
    if !enabled {
        return;
    }

    BOOT_REASON.store(
        if is_cold_boot {
            BootReason::Normal as u32
        } else {
            BootReason::None as u32
        },
        Ordering::Release,
    );

    // TIMG 看门狗交给管理引擎托管，先清掉中断再重新按需使能
    wdt::timg_clear_and_halt();

    defmt::info!("[ME] 管理引擎就绪（HP-Core 侧）");
}

/// 管理引擎是否在跑。
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// 装上温度传感器。
pub fn attach_sensor(peripheral: esp_hal::peripherals::TSENS<'static>) {
    let cfg = esp_hal::tsens::Config::default();
    match esp_hal::tsens::TemperatureSensor::new(peripheral, cfg) {
        Ok(sensor) => {
            critical_section::with(|cs| *SENSOR.borrow_ref_mut(cs) = Some(sensor));
            defmt::info!("[ME] 温度传感器就绪");
        }
        Err(_) => defmt::error!("[ME] 温度传感器初始化失败"),
    }
}

/// 采一次结温（摄氏度），传感器不可用时返回 0。
pub fn sample_temperature() -> u32 {
    let reading = critical_section::with(|cs| {
        SENSOR
            .borrow_ref(cs)
            .as_ref()
            .map(|s| s.get_temperature().to_celsius())
    });
    let c = reading.unwrap_or(0.0);
    let c = if c.is_finite() && c > 0.0 { c as u32 } else { 0 };
    TEMPERATURE.store(c, Ordering::Release);
    c
}

/// 最近一次采样的结温（摄氏度）。
pub fn temperature_c() -> u32 {
    TEMPERATURE.load(Ordering::Acquire)
}

/// 喂狗并检查温度阈值，超过紧急阈值时直接进 S5。
pub fn pet_watchdog() {
    if !is_active() || petting_suppressed() {
        return;
    }

    // 喂 LP 侧的软件看门狗：计数不变超过 15 s，LP 核会直接复位整芯片。
    // 这是 HP 核死锁时唯一的自救路径，所以必须放在所有阻塞循环里调用。
    let counter = shared_read(shared::WDT_COUNTER).wrapping_add(1);
    shared_write(shared::WDT_COUNTER, counter);

    let cfg = nvram::get();
    let temp = sample_temperature();


    update_governor(&cfg);

    if temp >= cfg.thermal_emergency_c as u32 && temp != 0 {
        crate::hal::console::print("\r\n[THERMAL_CRITICAL] 结温超过紧急阈值，进入 S5。\r\n");
        crate::hal::power::enter_s5();
    }
    if temp >= cfg.thermal_throttle_c as u32 && temp != 0 {
        crate::hal::clock::set_cpu_mhz_raw(crate::hal::clock::LOW_MHZ);
    }
}

/// 读启动原因。
pub fn boot_reason() -> BootReason {
    match BOOT_REASON.load(Ordering::Acquire) {
        1 => BootReason::Normal,
        2 => BootReason::Setup,
        3 => BootReason::ForceReset,
        4 => BootReason::Watchdog,
        5 => BootReason::Thermal,
        _ => BootReason::None,
    }
}

/// 置启动原因。
/// 清启动原因。
pub fn clear_boot_reason() {
    BOOT_REASON.store(BootReason::None as u32, Ordering::Release);
}

/// 调频决策周期（毫秒）。
const GOV_PERIOD_MS: u32 = 100;
/// 降频抖动抑制保持时间（毫秒）。
const GOV_DOWN_HOLD_MS: u32 = 1000;
/// 升到中频档的占用率阈值（百分比）。
const GOV_MID_LOAD: u32 = 20;
/// 升到高频档的占用率阈值（百分比）。
const GOV_HIGH_LOAD: u32 = 70;

/// 下一次调频决策的时刻（embassy 毫秒）。
static GOV_NEXT_MS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// 上一周期的忙计数。
static GOV_LAST_BUSY: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// 上一周期的闲计数。
static GOV_LAST_IDLE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// 当前已下发的频率（MHz）。
static GOV_APPLIED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// 降频抑制已累计的时间（毫秒）。
static GOV_HOLD_MS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// 自治调频：按忙闲比三档决策，升频立即生效、降频保持 1 秒。
///
/// 决策放在 HP 侧而不是上游的 LP 核：PCR 在 HP 外设空间，LP 核写不进去
/// （实测写入后 `clock::get_cpu_mhz()` 读回仍是原值）。
fn update_governor(cfg: &nvram::Config) {
    use core::sync::atomic::Ordering;

    let now = embassy_time::Instant::now().as_millis() as u32;
    if now < GOV_NEXT_MS.load(Ordering::Relaxed) {
        return;
    }
    GOV_NEXT_MS.store(now + GOV_PERIOD_MS, Ordering::Relaxed);

    let busy = shared_read(shared::HP_BUSY_TICKS);
    let idle = shared_read(shared::HP_IDLE_TICKS);
    let delta_busy = busy.wrapping_sub(GOV_LAST_BUSY.load(Ordering::Relaxed));
    let delta_idle = idle.wrapping_sub(GOV_LAST_IDLE.load(Ordering::Relaxed));
    GOV_LAST_BUSY.store(busy, Ordering::Relaxed);
    GOV_LAST_IDLE.store(idle, Ordering::Relaxed);

    let total = delta_busy.saturating_add(delta_idle);
    let load = if total > 0 {
        delta_busy.saturating_mul(100) / total
    } else {
        0
    };
    shared_write(shared::CPU_LOAD, load);

    let temp = sample_temperature();
    let max_mhz = cpu_freq_mhz(cfg.cpu_freq);
    let mut target = 80;

    if cfg.cpu_governor == nvram::CpuGovernor::Performance {
        target = max_mhz;
    } else if temp == 0 || temp < cfg.thermal_throttle_c as u32 {
        if load >= GOV_HIGH_LOAD {
            target = 160;
        } else if load >= GOV_MID_LOAD {
            target = 120;
        }
    }
    if target > max_mhz {
        target = max_mhz;
    }

    let applied = GOV_APPLIED.load(Ordering::Relaxed);
    if target > applied {
        GOV_APPLIED.store(target, Ordering::Relaxed);
        GOV_HOLD_MS.store(0, Ordering::Relaxed);
        crate::hal::clock::set_cpu_mhz_raw(target);
        shared_write(shared::TARGET_FREQ, target);
    } else if target < applied {
        let hold = GOV_HOLD_MS.load(Ordering::Relaxed) + GOV_PERIOD_MS;
        if hold >= GOV_DOWN_HOLD_MS {
            GOV_APPLIED.store(target, Ordering::Relaxed);
            GOV_HOLD_MS.store(0, Ordering::Relaxed);
            crate::hal::clock::set_cpu_mhz_raw(target);
            shared_write(shared::TARGET_FREQ, target);
        } else {
            GOV_HOLD_MS.store(hold, Ordering::Relaxed);
        }
    } else {
        GOV_HOLD_MS.store(0, Ordering::Relaxed);
    }
}
