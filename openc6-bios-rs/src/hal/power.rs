//! 电源管理与复位溯源。

use esp_hal::rtc_cntl::SocResetReason;

use super::gpio;

/// 复位原因，对应上游的 `esp_reset_reason_t` 子集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum ResetReason {
    /// 上电复位。
    PowerOn,
    /// 软件复位。
    Software,
    /// 深睡唤醒。
    DeepSleep,
    /// 硬件看门狗复位。
    Watchdog,
    /// 欠压复位。
    Brownout,
    /// USB-Serial-JTAG 复位（调试器烧录 / 复位按钮经 JTAG）。
    UsbJtag,
    /// 未知或未归类。
    Other,
}

/// 读复位原因。
pub fn reset_reason() -> ResetReason {
    match esp_hal::system::reset_reason() {
        Some(SocResetReason::ChipPowerOn) => ResetReason::PowerOn,
        Some(SocResetReason::CoreSw | SocResetReason::Cpu0Sw) => ResetReason::Software,
        Some(SocResetReason::CoreDeepSleep) => ResetReason::DeepSleep,
        Some(
            SocResetReason::CoreMwdt0
            | SocResetReason::CoreMwdt1
            | SocResetReason::CoreRtcWdt
            | SocResetReason::Cpu0Mwdt0
            | SocResetReason::Cpu0Mwdt1
            | SocResetReason::Cpu0RtcWdt
            | SocResetReason::SysRtcWdt
            | SocResetReason::SysSuperWdt,
        ) => ResetReason::Watchdog,
        Some(SocResetReason::SysBrownOut) => ResetReason::Brownout,
        Some(SocResetReason::CoreUsbUart | SocResetReason::CoreUsbJtag | SocResetReason::Cpu0JtagCpu) => {
            ResetReason::UsbJtag
        }
        _ => ResetReason::Other,
    }
}

/// 是否为冷启动（不是从深睡唤醒）。
pub fn is_cold_boot() -> bool {
    reset_reason() != ResetReason::DeepSleep
}

/// 是否为真正的上电复位。
///
/// 掉电恢复策略只该在真上电时生效；调试器复位、看门狗复位、深睡唤醒都不算。
pub fn is_power_on() -> bool {
    reset_reason() == ResetReason::PowerOn
}

/// 是否由 panic / CPU 异常复位。
pub fn is_panic_reset() -> bool {
    reset_reason() == ResetReason::Software
}

/// 是否由软件或深睡唤醒复位。
/// 把电源键虚拟地锁到 0V，sense 线开内部上拉。
pub fn init_runtime_pins() {
    gpio::set_output(gpio::PIN_PWR_BTN_GND);
    gpio::write_pin(gpio::PIN_PWR_BTN_GND, false);
    gpio::set_input(gpio::PIN_PWR_BTN_SENSE, true);
    boot_btn_init();
}

/// 检查 Clear CMOS 跳线是否短接：把 GPIO1 拉低，读 GPIO2 电平。
pub fn check_clear_cmos_jumper() -> bool {
    gpio::set_output(gpio::PIN_CMOS_GND);
    gpio::write_pin(gpio::PIN_CMOS_GND, false);
    gpio::set_input(gpio::PIN_CLEAR_NVRAM, true);

    // 等跳线电平稳定
    for _ in 0..100_000 {
        core::hint::spin_loop();
    }

    let shorted = !gpio::read_pin(gpio::PIN_CLEAR_NVRAM);
    gpio::set_disabled(gpio::PIN_CMOS_GND);
    shorted
}

/// 把 BOOT 键配成上拉输入。
pub fn boot_btn_init() {
    gpio::set_input(gpio::PIN_BOOT_BTN, true);
}

/// BOOT 键当前是否按下（低电平有效）。
pub fn is_boot_btn_pressed() -> bool {
    !gpio::read_pin(gpio::PIN_BOOT_BTN)
}

/// 测量电源键上电时被按住的毫秒数，最长 3000 ms。
pub fn measure_wake_hold_ms() -> u32 {
    if is_cold_boot() {
        return 0;
    }

    gpio::set_output(gpio::PIN_PWR_BTN_GND);
    gpio::write_pin(gpio::PIN_PWR_BTN_GND, false);
    gpio::set_input(gpio::PIN_PWR_BTN_SENSE, true);

    let mut hold_ms = 0u32;
    while !gpio::read_pin(gpio::PIN_PWR_BTN_SENSE) && hold_ms < 3000 {
        esp_hal::delay::Delay::new().delay_millis(10);
        hold_ms += 10;
    }
    hold_ms
}

/// 进 S5 软关机：保持电源键虚拟地，配置 sense 线低电平唤醒后深睡。
pub fn enter_s5() -> ! {
    gpio::set_output(gpio::PIN_PWR_BTN_GND);
    gpio::write_pin(gpio::PIN_PWR_BTN_GND, false);
    gpio::set_input(gpio::PIN_PWR_BTN_SENSE, true);

    // 直接用 RTC_CNTL 的 ext1 配置，绕开 esp-hal 的 sleep 驱动对 pin 类型的依赖。
    configure_ext1_wakeup();

    let lpwr = unsafe { esp_hal::peripherals::LPWR::steal() };
    let mut low_power = esp_hal::rtc_cntl::sleep::LowPower::new(lpwr);
    low_power.sleep_deep(esp_hal::rtc_cntl::sleep::RtcSleepConfig::default())
}

/// 进 S5 并挂一个绝对时间闹钟，到点芯片自己复位醒来。
///
/// 测试 `poweroff` 时用它：深睡会让 USB 整个断开、控制台消失，
/// 万一手边没有按键或接线，板子会一直睡着。定时唤醒让这条路径自带兜底。
///
/// `secs` 必须留够余量（≥ 2 秒），
/// 唤醒时刻太近时 `sleep_deep` 会 panic（睡眠切换来不及捕获）。
pub fn enter_s5_with_timer(secs: u64) -> ! {
    gpio::set_output(gpio::PIN_PWR_BTN_GND);
    gpio::write_pin(gpio::PIN_PWR_BTN_GND, false);
    gpio::set_input(gpio::PIN_PWR_BTN_SENSE, true);

    let lpwr = unsafe { esp_hal::peripherals::LPWR::steal() };
    let mut low_power = esp_hal::rtc_cntl::sleep::LowPower::new(lpwr);
    low_power.set_wakeup_deadline(
        esp_hal::time::Instant::now() + esp_hal::time::Duration::from_secs(secs),
    );
    low_power.sleep_deep(esp_hal::rtc_cntl::sleep::RtcSleepConfig::default())
}

/// 配置 ext1 在 GPIO4 任意低电平时唤醒。
///
/// shortcut: 只支持 C6 的 EXT1 单组位图语义，多组/高电平唤醒没实现，换芯片时重写。
fn configure_ext1_wakeup() {
    const RTC_CNTL_BASE: usize = 0x600B_0000;
    /// RTC_CNTL_EXT_WAKEUP_CONF_REG：ext1 触发级别（0 = 低电平）。
    const EXT_WAKEUP_CONF: usize = RTC_CNTL_BASE + 0x018;
    /// RTC_CNTL_EXT_WAKEUP1_REG：ext1 唤醒引脚位图。
    const EXT_WAKEUP1: usize = RTC_CNTL_BASE + 0x01C;
    /// RTC_CNTL_EXT_WAKEUP1_STATUS_CLR_REG：清 ext1 状态。
    const EXT_WAKEUP1_STATUS_CLR: usize = RTC_CNTL_BASE + 0x028;

    super::regs::write(EXT_WAKEUP_CONF, 0);
    super::regs::write(EXT_WAKEUP1, 1 << gpio::PIN_PWR_BTN_SENSE);
    super::regs::write(EXT_WAKEUP1_STATUS_CLR, 1);
}

/// 软件重启 SoC，不返回。
pub fn reboot() -> ! {
    esp_hal::system::software_reset()
}
