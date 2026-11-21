#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Duration, Timer};
use esp_hal::{Blocking, rmt::Rmt, time::Rate, timer::timg::TimerGroup};
use esp_hal_smartled::{RmtSmartLeds, buffer_size, color_order};
use panic_rtt_target as _;
use smart_leds_trait::{RGB8, SmartLedsWrite};
use trouble_host::prelude::*;

/// 板载 WS2812B 适配器：RMT 阻塞写、单颗灯、GRB 顺序
type Led = RmtSmartLeds<'static, { buffer_size::<RGB8>(1) }, Blocking, RGB8, color_order::Grb>;

/// BLE 控制器带 10 个命令缓冲
type BleController = ExternalController<esp_radio::ble::controller::BleConnector<'static>, 10>;

/// 广播名，手机端按这个名字发现设备
const DEVICE_NAME: &str = "ble-led";
/// 灯控服务 UUID：蓝牙 SIG 的 Automation IO Service
const SERVICE_UUID: u16 = 0x1815;
/// 灯输出特性 UUID：蓝牙 SIG 的 Light Output
const CHARACTERISTIC_UUID: u16 = 0x2BE2;
/// 开灯颜色（绿色，板载灯亮度不宜开满）
const LED_ON_RGB: RGB8 = RGB8 { r: 0, g: 32, b: 0 };
/// 关灯颜色
const LED_OFF_RGB: RGB8 = RGB8 { r: 0, g: 0, b: 0 };
/// 同时保持的连接数
const CONNECTIONS_MAX: usize = 1;
/// L2CAP 通道数，trouble-host 需要预留 ATT 之外的通道
const L2CAP_CHANNELS_MAX: usize = 3;

macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write($val);
        x
    }};
}

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    let peripherals = esp_hal::init(esp_hal::Config::default());

    esp_alloc::heap_allocator!(size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    // nanoESP32-C6 板载 WS2812B：DIN 接 GPIO8，颜色顺序 GRB
    let freq = Rate::from_mhz(80);
    let rmt = Rmt::new(peripherals.RMT, freq).expect("RMT init failed");
    let mut led = Led::new(
        esp_hal_smartled::WS2812B_TIMING,
        rmt.channel0,
        peripherals.GPIO8,
        freq,
    )
    .expect("LED init failed");
    set_led(&mut led, false);

    let connector = esp_radio::ble::controller::BleConnector::new(
        peripherals.BT,
        esp_radio::ble::Config::default(),
    )
    .expect("BLE controller init failed");
    let controller = ExternalController::new(connector);

    let resources = mk_static!(
        HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX>,
        HostResources::new()
    );
    let stack: &'static Stack<'static, BleController, DefaultPacketPool> = mk_static!(
        Stack<'static, BleController, DefaultPacketPool>,
        trouble_host::new(controller, resources).build()
    );

    spawner.spawn(ble_runner(stack.runner()).expect("spawn ble_runner"));
    spawner.spawn(ble_server(stack.peripheral(), led).expect("spawn ble_server"));

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

/// 让协议栈跑起来，跑到底才算失败
#[embassy_executor::task]
async fn ble_runner(mut runner: Runner<'static, BleController, DefaultPacketPool>) {
    runner.run().await.expect("BLE stack exited");
}

/// 广播 GATT 服务，把手机写入的 on/off 落到灯上
#[embassy_executor::task]
async fn ble_server(
    mut peripheral: Peripheral<'static, BleController, DefaultPacketPool>,
    mut led: Led,
) {
    let mut table: AttributeTable<'_, NoopRawMutex, 10> = AttributeTable::new();

    // Generic Access 服务（强制）
    let id: &[u8] = DEVICE_NAME.as_bytes();
    let appearance: &[u8] = &[0x80, 0x07];
    let mut svc = table.add_service(Service::new(0x1800u16));
    let _ = svc.add_characteristic_ro(0x2a00u16, id);
    let _ = svc.add_characteristic_ro(0x2a01u16, appearance);
    svc.build();

    // Generic Attribute 服务（强制）
    table.add_service(Service::new(0x1801u16));

    // 灯输出特性：值是可变的 3 字节文本，存 "on" 或 "off"
    let light_storage = mk_static!([u8; 3], *b"off");
    table
        .add_service(Service::new(SERVICE_UUID))
        .add_characteristic(
            CHARACTERISTIC_UUID,
            [CharacteristicProp::Read, CharacteristicProp::Write],
            *b"off",
            light_storage,
        )
        .build();

    let server =
        AttributeServer::<NoopRawMutex, DefaultPacketPool, 10, CONNECTIONS_MAX>::new(table);

    let mut adv_data = [0; 31];
    let adv_data_len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(DEVICE_NAME.as_bytes()),
        ],
        &mut adv_data[..],
    )
    .unwrap();

    let mut scan_data = [0; 31];
    let scan_data_len = AdStructure::encode_slice(
        &[AdStructure::CompleteLocalName(DEVICE_NAME.as_bytes())],
        &mut scan_data[..],
    )
    .unwrap();

    let mut on = false;

    loop {
        info!("BLE: advertising as {}", DEVICE_NAME);
        let acceptor = peripheral
            .advertise(
                &Default::default(),
                Advertisement::ConnectableScannableUndirected {
                    adv_data: &adv_data[..adv_data_len],
                    scan_data: &scan_data[..scan_data_len],
                },
            )
            .await
            .unwrap();

        let conn = acceptor.accept().await.unwrap();
        info!("BLE: connected, light is {}", state_text(on));
        let conn = conn.with_attribute_server(&server).unwrap();

        loop {
            match conn.next().await {
                GattConnectionEvent::Disconnected { reason } => {
                    info!("BLE: disconnected, reason {:?}", reason);
                    break;
                }
                GattConnectionEvent::Gatt {
                    event: GattEvent::Write(event),
                } => {
                    match event.with_data(|_, data| parse_command(data)) {
                        Some(state) => {
                            // accept 之后特性值就是手机写入的原始字节
                            event.accept().unwrap().send().await;
                            on = state;
                            set_led(&mut led, on);
                            info!("BLE: received \"{}\"", state_text(on));
                        }
                        None => {
                            // 拒绝写入，特性值保持不变，读回仍是真实状态
                            info!("BLE: ignoring invalid command, only \"on\"/\"off\" accepted");
                            event
                                .reject(AttErrorCode::VALUE_NOT_ALLOWED)
                                .unwrap()
                                .send()
                                .await;
                        }
                    }
                }
                GattConnectionEvent::Gatt {
                    event: GattEvent::Read(event),
                } => {
                    event.accept().unwrap();
                }
                _ => {}
            }
        }

        Timer::after(Duration::from_millis(100)).await;
    }
}

/// 把手机写入的字节解析成灯的开关指令，非法内容返回 `None`
fn parse_command(data: &[u8]) -> Option<bool> {
    if data == b"on" {
        Some(true)
    } else if data == b"off" {
        Some(false)
    } else {
        None
    }
}

/// 灯当前状态的文本表示，与手机写入特性时的内容一致
fn state_text(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// 按开关状态写板载 RGB 灯
fn set_led(led: &mut Led, on: bool) {
    let color = if on { LED_ON_RGB } else { LED_OFF_RGB };
    if led.write([color]).is_err() {
        error!("LED write failed");
    }
}
