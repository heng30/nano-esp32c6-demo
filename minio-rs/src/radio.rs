//! 射频租约的固件侧单例；逻辑在 core::lease，这里只包临界区。

use core::cell::RefCell;

use critical_section::Mutex;
use minio_rs_core::lease::{LeaseError, RadioKind, RadioLeases};

/// 全局租约管理器；menu 任务与各 app 任务经它仲裁射频
static RADIO: Mutex<RefCell<RadioLeases>> = Mutex::new(RefCell::new(RadioLeases::new()));

/// 取独占租约
pub fn acquire(kind: RadioKind) -> Result<(), LeaseError> {
    critical_section::with(|cs| RADIO.borrow_ref_mut(cs).acquire_exclusive(kind))
}

