#![no_std]
#![no_main]

extern crate alloc;

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::timer::timg::TimerGroup;

mod fs;
mod hal;
mod led;
mod me;
mod net;
mod nvram;
mod power;
mod proc;
mod pxe;
mod sandbox;
mod shell;
mod zc6;

esp_bootloader_esp_idf::esp_app_desc!();

/// BIOS 版本串，打印在 POST 头一行。
const BIOS_VERSION: &str = "OpenC6 BIOS v2.0-ME (Rust)";
/// BIOS Setup 的 AP SSID。
pub const BIOS_AP_SSID: &str = "BIOS_SETUP_C6";
/// BIOS Setup 的 AP 密码。
pub const BIOS_AP_PASS: &str = "12345678";
/// BOOT 键按住多久算「要进 Boot Menu」。
const BOOT_BTN_HOLD_MS: u32 = 1000;
/// 电源键按住多久算「要进 Setup」。
const PWR_BTN_SETUP_MS: u32 = 3000;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    hal::console::print("\r\n*** PANIC ***\r\n");
    if let Some(loc) = info.location() {
        defmt::error!("panic at {}:{}", loc.file(), loc.line());
    } else {
        defmt::error!("panic (no location)");
    }
    hal::power::reboot()
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default();
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 160 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    // USB CDC 必须最先起来，之后所有日志和 shell 都走它
    hal::console::init();
    hal::console::print("\r\n");

    let rst = hal::power::reset_reason();
    let is_cold_boot = hal::power::is_cold_boot();
    let legacy_hold_ms = hal::power::measure_wake_hold_ms();

    hal::power::init_runtime_pins();

    let me_reason = me::boot_reason();
    me::clear_boot_reason();

    hal::console::print(&alloc::format!(
        "--- {} (ME Reason: {:?}, RST Reason: {:?}) ---\r\n",
        BIOS_VERSION,
        me_reason,
        rst
    ));

    net::set_peripheral(peripherals.WIFI);

    hal::flash::init(peripherals.FLASH);
    nvram::init();
    led::init(peripherals.RMT, peripherals.GPIO8);
    fs::init();
    proc::init();

    if !zc6::self_check() {
        hal::console::print("[ZC6] 编解码器自检失败！\r\n");
        defmt::error!("ZC6 回环自检失败");
    }

    me::attach_sensor(peripherals.TSENS);
    me::init(true, is_cold_boot);

    // 电源键引脚必须配在 LP_IO 域，LP 核才读得到电平。
    // 而且必须在启动 LP 核**之前**配好：否则 LP 侧首轮读到 0 会把按键
    // 误判成常按，5 秒后触发强制复位，形成复位循环。
    me::config_power_button();

    // 管理引擎的 LP-Core 侧：装载镜像并启动
    if !me::start_lp_core(peripherals.LP_CORE) {
        hal::console::print("[ME] LP-Core 启动失败，回落到 HP-Core 监督\r\n");
    }

    // OTA 待处理：连 Wi-Fi 拉新镜像
    if nvram::get().bios_update_state == nvram::BiosUpdateState::Pending {
        nvram::update(|c| c.bios_update_state = nvram::BiosUpdateState::None);
        led::set_aura_mode(nvram::AuraMode::Disabled);
        led::set_color(0, 0, 255);

        if net::init(spawner).is_some() && net::start_sta() {
            Timer::after(Duration::from_secs(3)).await;
            let url = alloc::string::String::from(nvram::get().pxe_url_str());
            if pxe::bios_ota(&url).await.is_ok() {
                led::set_color(0, 255, 0);
                Timer::after(Duration::from_secs(1)).await;
                hal::power::reboot();
            }
            led::set_color(255, 0, 0);
            Timer::after(Duration::from_secs(2)).await;
        }
    }

    // Clear CMOS 跳线：恢复出厂设置后重启
    if hal::power::check_clear_cmos_jumper() {
        led::set_aura_mode(nvram::AuraMode::Disabled);
        led::set_color(led::COLOR_POST_ERROR.0, led::COLOR_POST_ERROR.1, led::COLOR_POST_ERROR.2);
        nvram::load_defaults();
        Timer::after(Duration::from_secs(2)).await;
        hal::power::reboot();
    }

    power::apply_bios_settings();
    led::blink_post(led::COLOR_POST_OK, 3);
    led::apply_aura_from_nvram();

    // 启动后台任务
    spawner.spawn(led::aura_task().unwrap());
    spawner.spawn(power::governor_task().unwrap());

    // BOOT 键按住：进 Boot Menu（这里简化为直接进 shell）
    let mut enter_shell = false;
    if hal::power::is_boot_btn_pressed() {
        let mut hold = 0u32;
        while hal::power::is_boot_btn_pressed() && hold < BOOT_BTN_HOLD_MS {
            Timer::after(Duration::from_millis(10)).await;
            hold += 10;
        }
        if hold >= BOOT_BTN_HOLD_MS {
            hal::console::print("[BOOT_MENU] BOOT 键按住，进入 Shell。\r\n");
            enter_shell = true;
        }
    }

    // 复位溯源
    if me_reason == me::BootReason::Thermal {
        hal::console::print("[THERMAL_CRITICAL] 结温超限，进 S5。\r\n");
        Timer::after(Duration::from_millis(100)).await;
        hal::power::enter_s5();
    } else if me_reason == me::BootReason::ForceReset {
        hal::console::print("[PWR_MGMT] 电源键长按强制关机，进 S5。\r\n");
        Timer::after(Duration::from_millis(100)).await;
        hal::power::enter_s5();
    } else if me_reason == me::BootReason::Setup || legacy_hold_ms >= PWR_BTN_SETUP_MS {
        hal::console::print("[BOOT_TRIGGER] 进入 BIOS Setup Utility。\r\n");
        enter_setup(spawner).await;
    } else if me_reason == me::BootReason::Watchdog || hal::power::is_panic_reset() {
        hal::console::print("[KERNEL_RECOVERY] 检测到异常复位，进 Shell。\r\n");
        enter_shell = true;
    } else if is_cold_boot
        && nvram::is_provisioned()
        && hal::power::is_power_on()
        && nvram::get().dc_loss_action == nvram::DcLossAction::PowerOff
        && me_reason == me::BootReason::None
    {
        hal::console::print("[PWR_MGMT] 冷上电且 DC_LOSS = POWER_OFF，进 S5。\r\n");
        Timer::after(Duration::from_millis(100)).await;
        hal::power::enter_s5();
    }

    // c6wsh 远程 shell：连 STA 并把控制台输出镜像到 Web
    if nvram::get().c6wsh_state == nvram::C6wshState::Enabled {
        hal::console::print("[C6WSH] NVRAM 已启用 Web Shell，连接 Wi-Fi...\r\n");
        if net::init(spawner).is_some() && net::start_sta() {
            hal::console::set_tap(Some(net::console_tap));
            hal::console::print(&alloc::format!(
                "  c6wsh READY: http://{}/wsh?cmd=help\r\n",
                net::ip_string()
            ));
            enter_shell = true;
        } else {
            hal::console::print("[C6WSH] STA 失败，回落到本地 USB shell。\r\n");
            enter_shell = true;
        }
    }

    // 配了 STA 凭据就开机联网：PXE / OTA / Web UI 都依赖这条链路。
    // `net::init` 内部已把凭据当作 `with_initial_config` 自动关联，不必再调 start_sta。
    if !nvram::get().ssid_str().is_empty() && net::stack_ptr_is_null() {
        net::init(spawner);
    }

    // 默认启动路径：没有强制进 shell 时，先试着跑 /downloaded/payload.bin
    if !enter_shell && try_boot_default().is_ok() {
        // payload 已跑完，落到 shell
    }

    let _ = spawner;
    shell::repl(spawner).await
}

/// 尝试从文件系统直接启动默认 payload。
fn try_boot_default() -> Result<(), ()> {
    if !hal::flash::is_ready() {
        return Err(());
    }

    let dir = fs::find_id("downloaded", 0).unwrap_or(0);
    for name in ["payload.bin", "payload.zc6"] {
        if let Some(id) = fs::find_id(name, dir) {
            if fs::get_type(id) == fs::TYPE_FILE && fs::get_size(id).unwrap_or(0) > 0 {
                hal::console::print(&alloc::format!(
                    "[BOOT] 找到 /downloaded/{name}，交给沙箱启动\r\n"
                ));
                return proc::spawn(name, dir, false).map(|_| ()).map_err(|_| ());
            }
        }
    }

    hal::console::print("[BOOT] 没有默认 payload，打开 Shell。\r\n");
    Err(())
}

/// 进 BIOS Setup：起 AP + Web UI，然后原地守着。
async fn enter_setup(spawner: Spawner) -> ! {
    led::set_aura_mode(nvram::get().aura_mode);
    if nvram::get().aura_mode != nvram::AuraMode::Rainbow {
        led::set_color(led::COLOR_BIOS_SETUP.0, led::COLOR_BIOS_SETUP.1, led::COLOR_BIOS_SETUP.2);
    }

    if net::init(spawner).is_some() {
        net::start_ap();
        hal::console::print(&alloc::format!(
            "[SETUP] AP `{BIOS_AP_SSID}` 已启动，浏览 http://{}/\r\n",
            net::AP_GATEWAY
        ));
    } else {
        hal::console::print("[SETUP] Wi-Fi 初始化失败\r\n");
    }

    loop {
        me::pet_watchdog();
        Timer::after(Duration::from_secs(1)).await;
    }
}
