//! BLE HID 键盘：0x1812 服务（Report Map / Report+Notify / 报告引用描述符 0x2908）。
//! 配对连接后，菜单短按把 "minio-rs" 打到对端。
//! 上游的 HID 欺骗是让受害者设备把板子当键盘——这里等价：正常 GATT HID 配对。

use core::sync::atomic::{AtomicBool, Ordering};

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_sync::{
    blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex},
    signal::Signal,
};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;
use trouble_host::prelude::*;

/// 打字请求：菜单短按置位
static TYPE_REQUEST: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// HID 服务是否已启动
pub static HID_UP: AtomicBool = AtomicBool::new(false);

/// HID Information：bcdHID 1.01、无国家码、normally connectable
const HID_INFO: [u8; 4] = [0x01, 0x01, 0x00, 0x02];
/// 最小 boot 键盘报告描述符：修饰键 8 位 + 保留字节 + 6 键滚动
const KEYBOARD_REPORT_MAP: [u8; 45] = [
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, //
    0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81,
    0x02, //
    0x95, 0x01, 0x75, 0x08, 0x81, 0x03, //
    0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65, 0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81,
    0x00, //
    0xC0,
];
/// 要打的字符串（HID usage 码）：m i n i o - r s 回车
const PHRASE: [u8; 9] = [0x10, 0x0C, 0x11, 0x0C, 0x12, 0x2D, 0x15, 0x16, 0x28];
/// 按键间隔
const KEY_DELAY: Duration = Duration::from_millis(30);
/// 报告引用描述符：report id 1、类型 input
const REPORT_REF: [u8; 2] = [1, 1];
/// 广播名
const DEVICE_NAME: &[u8] = b"minio-rs Keyboard";
/// 键盘外观（GAP Appearance 0x03C1）
const APPEARANCE_KEYBOARD: [u8; 2] = [0xC1, 0x03];
/// 出错退避
const ERR_BACKOFF: Duration = Duration::from_millis(500);

/// 请求打一遍短语
pub fn request_type() {
    TYPE_REQUEST.signal(());
}

/// 启动 HID 任务（BLE 栈必须已由其它 BLE app 拉起）
pub fn start(spawner: &Spawner) -> Result<(), &'static str> {
    if HID_UP.load(Ordering::Relaxed) {
        return Ok(());
    }
    let Some(stack) = crate::ble::stack() else {
        return Err("ble stack not running, enter another ble app first");
    };
    spawner.spawn(hid_task(stack).expect("spawn hid task"));
    HID_UP.store(true, Ordering::Relaxed);
    info!("hid keyboard service starting");
    Ok(())
}

/// HID 任务：GATT 表 + 广播 accept 循环，连接期间写事件与打字请求并行
#[embassy_executor::task]
async fn hid_task(stack: &'static crate::ble::BleStack) {
    let mut table: AttributeTable<'_, NoopRawMutex, 20> = AttributeTable::new();

    // Generic Access：键盘名 + 键盘外观
    let mut gap = table.add_service(Service::new(0x1800u16));
    let _ = gap.add_characteristic_ro(0x2a00u16, DEVICE_NAME);
    let _ = gap.add_characteristic_ro(0x2a01u16, &APPEARANCE_KEYBOARD);
    gap.build();
    // Generic Attribute（强制）
    table.add_service(Service::new(0x1801u16));

    // HID 服务
    let mut svc = table.add_service(Service::new(0x1812u16));
    let _ = svc.add_characteristic_ro(0x2A4Au16, &HID_INFO);
    let _ = svc.add_characteristic_ro(0x2A4Bu16, &KEYBOARD_REPORT_MAP);
    // Protocol Mode：report 协议
    let _ = svc.add_characteristic_ro(0x2A4Eu16, &[0x01u8]);
    // 输入报告：Read + Notify，带报告引用描述符
    static REPORT_STORAGE: StaticCell<[u8; 8]> = StaticCell::new();
    let report_storage = REPORT_STORAGE.init([0u8; 8]);
    let mut report = svc.add_characteristic(
        0x2A4Du16,
        [CharacteristicProp::Read, CharacteristicProp::Notify],
        [0u8; 8],
        report_storage,
    );
    let _ = report.add_descriptor_ro(0x2908u16, PermissionLevel::Allowed, &REPORT_REF);
    let report_handle = report.build().handle();
    svc.build();

    let server = AttributeServer::<NoopRawMutex, DefaultPacketPool, 20, 1>::new(table);

    let mut peripheral = stack.peripheral();
    loop {
        // 广播：名字 + HID 服务 UUID
        let mut adv = [0u8; 31];
        let structures = [
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(DEVICE_NAME),
            AdStructure::CompleteServiceUuids16(&[[0x12, 0x18]]),
        ];
        let n = match AdStructure::encode_slice(&structures, &mut adv) {
            Ok(n) => n,
            Err(_) => {
                Timer::after(ERR_BACKOFF).await;
                continue;
            }
        };
        let advertiser = match peripheral
            .advertise(
                &Default::default(),
                Advertisement::ConnectableScannableUndirected {
                    adv_data: &adv[..n],
                    scan_data: &[],
                },
            )
            .await
        {
            Ok(a) => a,
            Err(e) => {
                warn!("hid advertise failed: {}", defmt::Debug2Format(&e));
                Timer::after(ERR_BACKOFF).await;
                continue;
            }
        };
        let conn = match advertiser.accept().await {
            Ok(c) => c,
            Err(e) => {
                warn!("hid accept failed: {}", defmt::Debug2Format(&e));
                continue;
            }
        };
        info!("hid keyboard connected");
        let conn = match conn.with_attribute_server(&server) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // 连接期间：写事件（Drop 自动接受）与打字请求并行
        loop {
            match select(conn.next(), TYPE_REQUEST.wait()).await {
                Either::First(GattConnectionEvent::Disconnected { reason }) => {
                    info!(
                        "hid keyboard disconnected: {}",
                        defmt::Debug2Format(&reason)
                    );
                    break;
                }
                Either::First(_) => {}
                Either::Second(()) => {
                    type_phrase(&server, stack, report_handle).await;
                }
            }
        }
    }
}

/// 逐键打出 PHRASE：按下→释放各发一次 notify
async fn type_phrase(
    server: &AttributeServer<'_, NoopRawMutex, DefaultPacketPool, 20, 1>,
    stack: &'static crate::ble::BleStack,
    handle: u16,
) {
    info!("hid typing phrase");
    for &code in PHRASE.iter() {
        let press = [0u8, 0, code, 0, 0, 0, 0, 0];
        if let Err(e) = server.notify(stack, handle, &press).await {
            warn!("notify failed: {}", defmt::Debug2Format(&e));
            return;
        }
        Timer::after(KEY_DELAY).await;
        if let Err(e) = server.notify(stack, handle, &[0u8; 8]).await {
            warn!("notify failed: {}", defmt::Debug2Format(&e));
            return;
        }
        Timer::after(KEY_DELAY).await;
    }
}
