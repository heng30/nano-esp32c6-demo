#![no_std]
#![no_main]

use esp_hal::{
    delay::Delay,
    rmt::Rmt,
    time::Rate,
};
use esp_hal_smartled::{RmtSmartLeds, buffer_size, color_order};
use smart_leds_trait::{RGB8, SmartLedsWrite};

esp_bootloader_esp_idf::esp_app_desc!();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    esp_hal::system::software_reset()
}

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    let freq = Rate::from_mhz(80);
    let rmt = Rmt::new(peripherals.RMT, freq).expect("RMT init failed");

    // nanoESP32-C6: 板载 WS2812B，DIN 接 GPIO8，颜色顺序 GRB
    let mut led = RmtSmartLeds::<{ buffer_size::<RGB8>(1) }, _, RGB8, color_order::Grb>::new(
        esp_hal_smartled::WS2812B_TIMING,
        rmt.channel0,
        peripherals.GPIO8,
        freq,
    )
    .expect("LED init failed");

    let delay = Delay::new();
    let mut hue: u8 = 0;

    loop {
        let (r, g, b) = hsv_to_rgb(hue, 255, 40);
        led.write([RGB8 { r, g, b }].into_iter()).unwrap();
        hue = hue.wrapping_add(2);
        delay.delay_millis(20);
    }
}

/// 把 HSV（色相 0..=255 一圈、饱和/明度 0..=255）转换成 RGB，用于彩虹循环。
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
