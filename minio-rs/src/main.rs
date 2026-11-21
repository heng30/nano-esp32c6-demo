//! minio-rs：Minino OS 的 Rust 移植（可行子集），功能对照见 docs/PORTING.md。
#![no_std]
#![no_main]

mod app;
#[cfg(feature = "wifi-build")]
mod ble;
#[cfg(feature = "wifi-build")]
mod hid;
#[cfg(feature = "wifi-build")]
mod net;
#[cfg(feature = "ieee154-build")]
mod radio154;
mod oled;
mod ota;
mod radio;
mod sd;
mod settings;

extern crate alloc;

use defmt::{info, warn};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::{Input, InputConfig, Pull};
use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
use esp_hal::time::{Duration as HalDuration, Instant as HalInstant, Rate};
use esp_hal::{rmt::Rmt, Blocking};
use esp_hal_smartled::{buffer_size, color_order, RmtSmartLeds};
use minio_rs_core::menu::{self, MenuId, MenuState, NavAction};
use panic_rtt_target as _;
use smart_leds_trait::{RGB8, SmartLedsWrite};

esp_bootloader_esp_idf::esp_app_desc!();

/// 板载 WS2812B 适配器：RMT 阻塞写、单颗灯、GRB 顺序
type Led = RmtSmartLeds<'static, { buffer_size::<RGB8>(1) }, Blocking, RGB8, color_order::Grb>;

/// 按键轮询周期
const POLL_PERIOD: Duration = Duration::from_millis(10);
/// 长按判定阈值，低于此视为短按
const LONG_PRESS: Duration = Duration::from_millis(600);
/// SetLightSleep 的睡眠时长
const LIGHT_SLEEP_SECS: u64 = 10;

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    let peripherals = esp_hal::init(esp_hal::Config::default());

    // 双堆布局照抄 ble-led：WiFi+BLE blob 与应用分配分开
    esp_alloc::heap_allocator!(size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    // 板载 BOOT 键（GPIO9，低电平按下），是菜单的唯一物理输入
    let boot = Input::new(peripherals.GPIO9, InputConfig::default().with_pull(Pull::Up));

    // WIFI 外设单独持有，captive portal / wardriving 首次进入时移交给网络栈
    #[cfg(feature = "wifi-build")]
    let mut wifi = Some(peripherals.WIFI);
    // BT 外设单独持有，BLE 首次进入时移交给 trouble-host
    #[cfg(feature = "wifi-build")]
    let mut bt = Some(peripherals.BT);
    // 802.15.4 外设单独持有，嗅探首次进入时移交给驱动
    #[cfg(feature = "ieee154-build")]
    let mut ieee154 = Some(peripherals.IEEE802154);

    // 偏好加载（nvs 分区第 0 扇区）
    settings::init(peripherals.FLASH);

    // 轻睡眠驱动，SetLightSleep 用
    let mut low_power = LowPower::new(peripherals.LPWR);

    // 板载 RGB 状态灯：非隐身时暗绿常亮，隐身时熄灭
    let freq = Rate::from_mhz(80);
    let rmt = Rmt::new(peripherals.RMT, freq).expect("RMT init failed");
    let mut led = Led::new(
        esp_hal_smartled::WS2812B_TIMING,
        rmt.channel0,
        peripherals.GPIO8,
        freq,
    )
    .expect("LED init failed");

    // 启动即验树：父节点/重复 id/清单完整性，坏了先报再继续
    match menu::verify_tree() {
        Ok(count) => info!("menu tree ok: {} entries", count),
        Err(e) => warn!("menu tree broken: {}", e),
    }

    // OLED：屏不在则降级纯日志（SDA=GPIO19、SCL=GPIO18，接线见 README）
    let mut screen = None;
    if let Ok(i2c) = esp_hal::i2c::master::I2c::new(
        peripherals.I2C0,
        esp_hal::i2c::master::Config::default()
            .with_frequency(esp_hal::time::Rate::from_khz(oled::I2C_FREQ_KHZ)),
    ) {
        let i2c = i2c.with_sda(peripherals.GPIO19).with_scl(peripherals.GPIO18);
        match oled::init(i2c) {
            Ok(d) => {
                info!("oled ready");
                screen = Some(d);
            }
            Err(e) => {
                info!("oled absent: {}", e);
            }
        }
    } else {
        info!("oled absent: i2c init failed");
    }

    // microSD：卡不在降级纯日志（接线见 README，CS=GPIO4、SCK=GPIO3、MISO=GPIO2、MOSI=GPIO1）
    if let Ok(spi) = esp_hal::spi::master::Spi::new(
        peripherals.SPI2,
        esp_hal::spi::master::Config::default()
            .with_frequency(esp_hal::time::Rate::from_khz(sd::SPI_FREQ_KHZ)),
    ) {
        let spi = spi
            .with_sck(peripherals.GPIO3)
            .with_mosi(peripherals.GPIO1)
            .with_miso(peripherals.GPIO2);
        let cs = esp_hal::gpio::Output::new(
            peripherals.GPIO4,
            esp_hal::gpio::Level::High,
            esp_hal::gpio::OutputConfig::default(),
        );
        match sd::init(spi, cs) {
            Ok(()) => match sd::with_sd(|s| s.capacity_bytes() / 1024 / 1024) {
                Some(mb) => info!("sd ready: {} MB", mb),
                None => info!("sd init reported ok but handle missing"),
            },
            Err(e) => info!("sd absent: {}", e),
        }
    }

    let mut state = MenuState::default();
    let mut in_app: Option<MenuId> = None;
    show_list(&mut screen, &state);

    // 按键状态机：下降沿记时刻，上升沿按时长分类短/长按
    let mut released = true;
    let mut press_at: Option<Instant> = None;
    // 屏保：空闲超时清屏一次，任意按键活动唤醒
    let mut last_input = Instant::now();
    let mut blanked = false;
    // LED 已应用的隐身状态缓存，避免每轮重写 RMT
    let mut led_stealth: Option<bool> = None;
    loop {
        Timer::after(POLL_PERIOD).await;

        // 隐身模式 → 状态灯
        let stealth = settings::current().stealth;
        if led_stealth != Some(stealth) {
            let color = if stealth {
                RGB8::new(0, 0, 0)
            } else {
                RGB8::new(0, 24, 0)
            };
            let _ = led.write([color]);
            led_stealth = Some(stealth);
        }

        // 屏保：空闲超时清屏
        let saver = settings::current().screensaver_secs;
        if saver > 0
            && !blanked
            && Instant::now() - last_input >= Duration::from_secs(saver as u64)
        {
            oled::blank(&mut screen);
            blanked = true;
        }

        let pressed = boot.is_low();
        if released && pressed {
            press_at = Some(Instant::now());
            last_input = Instant::now();
            blanked = false;
        } else if !released && !pressed {
            let long = press_at
                .map(|t| Instant::now() - t >= LONG_PRESS)
                .unwrap_or(false);
            press_at = None;
            if long {
                // 长按：应用内退出；列表内进入
                if let Some(id) = in_app {
                    app::on_exit(id);
                    in_app = None;
                    show_list(&mut screen, &state);
                } else {
                    match state.enter() {
                        NavAction::Drill => show_list(&mut screen, &state),
                        NavAction::App(id) => {
                            app::on_enter(id);
                            in_app = Some(id);
                            info!("long-press BOOT to exit");
                            // portal / wardriving 首次进入时拉起网络栈（常驻到重启）
                            #[cfg(feature = "wifi-build")]
                            if matches!(
                                id,
                                MenuId::WifiCaptivePortal | MenuId::WifiWardriving
                            ) {
                                if let Some(wifi) = wifi.take() {
                                    if let Err(e) = net::start(&spawner, wifi) {
                                        warn!("wifi start failed: {}", e);
                                    }
                                }
                            }
                            // BLE app 首次进入时拉起 trouble-host 栈（常驻到重启）
                            #[cfg(feature = "wifi-build")]
                            if matches!(
                                id,
                                MenuId::BleTrackers
                                    | MenuId::BleAdvSniffer
                                    | MenuId::Surveillance
                                    | MenuId::BleSpammer
                                    | MenuId::BleHid
                            ) {
                                if let Some(bt) = bt.take() {
                                    if let Err(e) = ble::start(&spawner, bt) {
                                        warn!("ble start failed: {}", e);
                                    }
                                }
                            }
                            // HID 服务在栈起来后单独拉起
                            #[cfg(feature = "wifi-build")]
                            if id == MenuId::BleHid
                                && !hid::HID_UP.load(core::sync::atomic::Ordering::Relaxed)
                            {
                                if let Err(e) = hid::start(&spawner) {
                                    warn!("hid start failed: {}", e);
                                }
                            }
                            #[cfg(feature = "ieee154-build")]
                            if matches!(id, MenuId::Sniffer154 | MenuId::Sniffer6LoWpan)
                                && !radio154::SNIFFER_UP
                                    .load(core::sync::atomic::Ordering::Relaxed)
                            {
                                let _ =
                                    radio::acquire(minio_rs_core::lease::RadioKind::Ieee802154);
                                if let Some(p) = ieee154.take() {
                                    let driver =
                                        esp_radio::ieee802154::Ieee802154::new(p);
                                    spawner.spawn(
                                        radio154::sniffer(driver).expect("spawn sniffer"),
                                    );
                                }
                            }
                            // SD 状态在进入时报一次
                            if id == MenuId::SetSdCard {
                                match sd::with_sd(|s| s.capacity_bytes() / 1024 / 1024) {
                                    Some(mb) => info!("sd mounted: {} MB", mb),
                                    None => info!("sd absent"),
                                }
                            }
                        }
                        NavAction::Nothing => {}
                    }
                }
            } else if let Some(id) = in_app {
                // 短按：应用内派发；SD 自检与轻睡眠在 main 处理（要拿本地资源）
                if id == MenuId::SetSdCard {
                    let r = sd::with_sd(|s| {
                        s.write_new_file("MINIO.TXT", b"minio-rs sd ok\n")
                    });
                    match r {
                        Some(Ok(())) => {}
                        Some(Err(e)) => warn!("sd selftest failed: {}", e),
                        None => info!("sd absent, nothing to test"),
                    }
                } else if id == MenuId::SetLightSleep {
                    info!("light sleep {} s", LIGHT_SLEEP_SECS);
                    low_power.set_wakeup_deadline(
                        HalInstant::now() + HalDuration::from_secs(LIGHT_SLEEP_SECS),
                    );
                    low_power.sleep_light(RtcSleepConfig::default());
                    info!("woken up from light sleep");
                } else {
                    app::on_input(id);
                }
            } else {
                // 短按：列表内下一项
                state.next();
                show_list(&mut screen, &state);
            }
        }
        released = !pressed;
    }
}

/// 渲染当前列表：OLED 优先（无屏空操作），RTT 日志始终保留
fn show_list(screen: &mut Option<oled::Display>, state: &MenuState) {
    let title = state.parent().map(menu::name).unwrap_or("minio-rs");
    // 最大层（Applications）8 项，12 容量留余
    let mut names: [&str; 12] = [""; 12];
    let mut n = 0;
    for m in menu::children(state.parent()) {
        if n < names.len() {
            names[n] = m.name;
            n += 1;
        }
    }
    oled::render_list(
        screen,
        settings::current().menu_style,
        title,
        &names[..n],
        state.pos_of(),
    );
    info!("== {} ==", title);
    for (i, m) in menu::children(state.parent()).enumerate() {
        let cursor = if i == state.pos_of() { ">" } else { " " };
        info!("{} {}", cursor, m.name);
    }
}
