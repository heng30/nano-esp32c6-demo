#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use defmt::info;
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Duration, Timer};
use esp_hal::timer::timg::TimerGroup;
use panic_rtt_target as _;
use trouble_host::prelude::*;

esp_bootloader_esp_idf::esp_app_desc!();

const DEVICE_NAME: &str = "ble-echo";
const PASSKEY: u32 = 123456;

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 3;

macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default();
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!("Embassy initialized!");

    info!("Starting BLE...");
    let ble_config = esp_radio::ble::Config::default();
    let connector =
        esp_radio::ble::controller::BleConnector::new(peripherals.BT, ble_config).unwrap();

    let controller = ExternalController::new(connector);
    info!("BLE controller initialized!");

    let resources = mk_static!(
        HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX>,
        HostResources::new()
    );

    let stack: &'static Stack<'static, BleController, DefaultPacketPool> = mk_static!(
        Stack<'static, BleController, DefaultPacketPool>,
        trouble_host::new(controller, resources)
            .set_random_address(Address::random([0xff, 0x9f, 0x1a, 0x05, 0xe4, 0xff]))
            .build()
    );

    stack.set_io_capabilities(IoCapabilities::DisplayOnly);

    let runner = stack.runner();
    let peripheral = stack.peripheral();

    info!("BLE host stack built!");

    spawner.spawn(ble_runner(runner).unwrap());
    spawner.spawn(ble_server(peripheral).expect("spawn ble_server"));

    loop {
        Timer::after(Duration::from_secs(30)).await;
        info!("Running...");
    }
}

type BleController = ExternalController<esp_radio::ble::controller::BleConnector<'static>, 10>;

#[embassy_executor::task]
async fn ble_runner(mut runner: Runner<'static, BleController, DefaultPacketPool>) {
    runner.run().await.ok();
}

#[embassy_executor::task]
async fn ble_server(mut peripheral: Peripheral<'static, BleController, DefaultPacketPool>) {
    let mut table: AttributeTable<'_, NoopRawMutex, 10> = AttributeTable::new();

    // Generic Access service (mandatory)
    let id: &[u8] = DEVICE_NAME.as_bytes();
    let appearance: &[u8] = &[0x80, 0x07];
    let mut svc = table.add_service(Service::new(0x1800u16));
    let _ = svc.add_characteristic_ro(0x2a00u16, id);
    let _ = svc.add_characteristic_ro(0x2a01u16, appearance);
    svc.build();

    // Generic Attribute service (mandatory)
    table.add_service(Service::new(0x1801u16));

    // Echo Service — Read/Write/Notify for bidirectional data transfer
    let echo_service_uuid: Uuid = Uuid::new_long([
        0x00, 0x00, 0x10, 0x00, 0xb0, 0xcd, 0x11, 0xec, 0x87, 0x1f, 0xd4, 0x5d, 0xdf, 0x13, 0x88,
        0x40,
    ]);
    let echo_char_uuid: Uuid = Uuid::new_long([
        0x00, 0x00, 0x10, 0x01, 0xb0, 0xcd, 0x11, 0xec, 0x87, 0x1f, 0xd4, 0x5d, 0xdf, 0x13, 0x88,
        0x40,
    ]);

    // Static storage for the echo characteristic value
    let echo_storage = mk_static!([u8; 20], [0u8; 20]);

    let echo_handle: Characteristic<[u8; 20]> = table
        .add_service(Service::new(echo_service_uuid))
        .add_characteristic(
            echo_char_uuid,
            &[
                CharacteristicProp::Read,
                CharacteristicProp::Write,
                CharacteristicProp::Notify,
            ],
            [0u8; 20],
            echo_storage,
        )
        .build();

    let server =
        AttributeServer::<NoopRawMutex, DefaultPacketPool, 10, CONNECTIONS_MAX>::new(table);

    info!("GATT server configured!");

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

    loop {
        info!("BLE: advertising...");
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
        info!("BLE: client connected!");

        // Request security (pairing) immediately after connection.
        // This sends a Security Request to the central (phone),
        // which will trigger passkey entry on the phone side.
        conn.request_security().ok();

        let conn = conn.with_attribute_server(&server).unwrap();
        info!(
            "BLE: GATT connection established, default passkey: {}",
            PASSKEY
        );

        loop {
            match conn.next().await {
                GattConnectionEvent::Disconnected { reason } => {
                    info!("BLE: client disconnected, reason: {:?}", reason);
                    break;
                }
                GattConnectionEvent::Gatt {
                    event: GattEvent::Write(event),
                } => {
                    // Client wrote data — log it and echo back via notification
                    info!(
                        "BLE: received {} bytes",
                        event.with_data(|_, data| data.len())
                    );

                    // Accept the write (updates the characteristic value)
                    event.accept().unwrap().send().await;

                    // Read back the written value and notify the client
                    let value: [u8; 20] = server.table().get(&echo_handle).unwrap();
                    info!("BLE: echoing back via notification");
                    echo_handle.notify(&conn, &value, false).await.ok();
                }
                GattConnectionEvent::Gatt {
                    event: GattEvent::Read(event),
                } => {
                    info!("BLE: read request");
                    event.accept().unwrap();
                }
                GattConnectionEvent::PassKeyInput => {
                    info!("BLE: passkey input requested, sending {}", PASSKEY);
                    conn.pass_key_input(PASSKEY).unwrap();
                }
                GattConnectionEvent::PassKeyDisplay(key) => {
                    info!("BLE: passkey displayed: {:06}", key.value());
                    info!(">>> Enter this 6-digit passkey on your phone <<<");
                }
                GattConnectionEvent::PassKeyConfirm(key) => {
                    info!("BLE: passkey confirm: {:06}", key.value());
                    conn.pass_key_confirm().unwrap();
                }
                GattConnectionEvent::PairingComplete {
                    security_level,
                    bond,
                } => {
                    info!(
                        "BLE: pairing complete, security: {:?}, bonded: {}",
                        security_level,
                        bond.is_some()
                    );
                }
                GattConnectionEvent::PairingFailed(err) => {
                    info!("BLE: pairing failed: {:?}", err);
                }
                _ => {}
            }
        }

        Timer::after(Duration::from_millis(100)).await;
    }
}
