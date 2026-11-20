//! 板载 WS2812B 状态灯：POST 诊断、静态颜色与 Aura Sync 灯效。

use core::{cell::RefCell, sync::atomic::{AtomicU8, Ordering}};

use critical_section::Mutex;
use esp_hal::{
    rmt::Rmt,
    time::Rate,
};
use esp_hal_smartled::{RmtSmartLeds, buffer_size, color_order};
use smart_leds_trait::{RGB8, SmartLedsWrite};

use crate::nvram::{self, AuraMode};

/// RMT 时基频率，WS2812B 的 0.3/0.9 µs 时序需要 80 MHz 分辨率。
const RMT_MHZ: u32 = 80;

/// POST 通过：绿色。
pub const COLOR_POST_OK: (u8, u8, u8) = (0, 255, 0);
/// POST 失败：红色。
pub const COLOR_POST_ERROR: (u8, u8, u8) = (255, 0, 0);
/// BIOS Setup 模式：蓝色。
pub const COLOR_BIOS_SETUP: (u8, u8, u8) = (0, 0, 255);

type Leds = RmtSmartLeds<'static, { buffer_size::<RGB8>(1) }, esp_hal::Blocking, RGB8, color_order::Grb>;

static LEDS: Mutex<RefCell<Option<Leds>>> = Mutex::new(RefCell::new(None));
/// 当前灯效档位，Aura 任务每次循环读一次。
static MODE: AtomicU8 = AtomicU8::new(AuraMode::Static as u8);

/// 初始化 RMT 与 WS2812B 通道。
pub fn init(rmt: esp_hal::peripherals::RMT<'static>, pin: esp_hal::peripherals::GPIO8<'static>) {
    let freq = Rate::from_mhz(RMT_MHZ);
    let Ok(rmt) = Rmt::new(rmt, freq) else {
        defmt::error!("[LED] RMT 初始化失败");
        return;
    };
    let Ok(led) = RmtSmartLeds::<{ buffer_size::<RGB8>(1) }, _, RGB8, color_order::Grb>::new(
        esp_hal_smartled::WS2812B_TIMING,
        rmt.channel0,
        pin,
        freq,
    ) else {
        defmt::error!("[LED] WS2812 通道初始化失败");
        return;
    };
    critical_section::with(|cs| *LEDS.borrow_ref_mut(cs) = Some(led));}

fn write_raw(r: u8, g: u8, b: u8) {
    critical_section::with(|cs| {
        if let Some(leds) = LEDS.borrow_ref_mut(cs).as_mut() {
            let _ = leds.write([RGB8 { r, g, b }].into_iter());
        }
    });
}

/// 按当前 NVRAM 亮度缩放后输出颜色。
pub fn set_color(r: u8, g: u8, b: u8) {
    let brightness = nvram::get().aura_brightness;
    if r == 0 && g == 0 && b == 0 {
        write_raw(0, 0, 0);
        return;
    }
    let scale = |c: u8| ((c as u16 * brightness as u16) / 255) as u8;
    write_raw(scale(r), scale(g), scale(b));
}

/// 同步闪烁若干次，用于 POST 码。
pub fn blink_post(color: (u8, u8, u8), count: u32) {
    if nvram::get().post_led_mode != nvram::PostLedMode::Enabled {
        return;
    }
    let delay = esp_hal::delay::Delay::new();
    for _ in 0..count {
        set_color(color.0, color.1, color.2);
        delay.delay_millis(120);
        set_color(0, 0, 0);
        delay.delay_millis(120);
    }
}

/// 切换 Aura 灯效档位。`Rainbow` 由 [`aura_task`] 驱动，其余直接落色。
pub fn set_aura_mode(mode: AuraMode) {
    MODE.store(mode as u8, Ordering::Release);
    match mode {
        AuraMode::Disabled => write_raw(0, 0, 0),
        AuraMode::Static => set_color(COLOR_BIOS_SETUP.0, COLOR_BIOS_SETUP.1, COLOR_BIOS_SETUP.2),
        AuraMode::Rainbow => {}
    }
}

/// 从 NVRAM 恢复灯效设置。
pub fn apply_aura_from_nvram() {
    set_aura_mode(nvram::get().aura_mode);
}

/// Aura 背景动画：彩虹循环，档位切走后自动停手。
#[embassy_executor::task]
pub async fn aura_task() {
    let mut hue = 0u8;
    loop {
        if MODE.load(Ordering::Acquire) == AuraMode::Rainbow as u8 {
            let (r, g, b) = hsv_to_rgb(hue, 255, 40);
            set_color(r, g, b);
            hue = hue.wrapping_add(2);
        }
        embassy_time::Timer::after(embassy_time::Duration::from_millis(20)).await;
    }
}

/// 把 HSV（色相 0..=255 一圈、饱和/明度 0..=255）转成 RGB。
fn hsv_to_rgb(h: u8, s: u8, v: u8) -> (u8, u8, u8) {
    let region = (h as u16 * 6) / 256;
    let rem = ((h as u16 * 6) % 256) as u16;
    let p = (v as u16 * (255 - s as u16)) / 255;
    let q = (v as u16 * (255 - (s as u16 * rem) / 256)) / 255;
    let t = (v as u16 * (255 - (s as u16 * (256 - rem)) / 256)) / 255;
    match region {
        0 => (v, t as u8, p as u8),
        1 => (q as u8, v, p as u8),
        2 => (p as u8, v, t as u8),
        3 => (p as u8, q as u8, v),
        4 => (t as u8, p as u8, v),
        _ => (v, p as u8, q as u8),
    }
}
