//! 射频租约仲裁器：上游 `components/radio_lease` 的等价物。
//! 独占租约（WiFi / 802.15.4 / BLE）互斥 + 引用计数；共享 promiscuous 租约
//! 只保留枚举位——esp-radio 无 promiscuous 能力，acquire 一律 Unsupported。

/// 独占射频种类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioKind {
    Wifi,
    Ieee802154,
    Ble,
}

/// 共享 promiscuous RX 租约，对应上游 `RADIO_PROM_*`；当前全部不可用
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromKind {
    Spectrum,
    Surveillance,
    Analyzer,
}

/// 租约错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseError {
    /// 射频被其它协议占用
    Busy(RadioKind),
    /// esp-radio blob 没有 promiscuous API
    Unsupported,
}

/// 引用计数上限；实际同时持有租约的代码路径远少于这个数
const MAX_REFS: u8 = 8;

/// 租约管理器；固件内放 `static`，由临界区保护调用
#[derive(Debug, Default)]
pub struct RadioLeases {
    /// 当前独占的射频种类；None = 空闲
    current: Option<RadioKind>,
    /// 独占租约引用计数
    refs: u8,
}

impl RadioLeases {
    /// 全空闲
    pub const fn new() -> Self {
        Self { current: None, refs: 0 }
    }

    /// 取独占租约：同种类叠加计数，异种类报 Busy
    pub fn acquire_exclusive(&mut self, kind: RadioKind) -> Result<(), LeaseError> {
        match self.current {
            Some(c) if c != kind => Err(LeaseError::Busy(c)),
            Some(_) if self.refs >= MAX_REFS => Err(LeaseError::Busy(kind)),
            Some(_) => {
                self.refs += 1;
                Ok(())
            }
            None => {
                self.current = Some(kind);
                self.refs = 1;
                Ok(())
            }
        }
    }

    /// 释放一次独占租约；计数归零后射频空闲
    pub fn release_exclusive(&mut self, kind: RadioKind) {
        if self.current == Some(kind) && self.refs > 0 {
            self.refs -= 1;
            if self.refs == 0 {
                self.current = None;
            }
        }
    }

    /// 当前占用者
    pub fn current(&self) -> Option<RadioKind> {
        self.current
    }

    /// 取共享 promiscuous 租约；底层能力缺失，恒 Unsupported
    pub fn acquire_promiscuous(&mut self, _kind: PromKind) -> Result<(), LeaseError> {
        Err(LeaseError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_mutual_exclusion() {
        let mut l = RadioLeases::new();
        assert_eq!(l.acquire_exclusive(RadioKind::Wifi), Ok(()));
        assert_eq!(
            l.acquire_exclusive(RadioKind::Ble),
            Err(LeaseError::Busy(RadioKind::Wifi))
        );
        assert_eq!(
            l.acquire_exclusive(RadioKind::Ieee802154),
            Err(LeaseError::Busy(RadioKind::Wifi))
        );
    }

    #[test]
    fn refcount_and_release() {
        let mut l = RadioLeases::new();
        l.acquire_exclusive(RadioKind::Wifi).unwrap();
        l.acquire_exclusive(RadioKind::Wifi).unwrap();
        l.release_exclusive(RadioKind::Wifi);
        // 还剩一个引用：仍占用
        assert_eq!(l.current(), Some(RadioKind::Wifi));
        assert_eq!(
            l.acquire_exclusive(RadioKind::Ble),
            Err(LeaseError::Busy(RadioKind::Wifi))
        );
        l.release_exclusive(RadioKind::Wifi);
        assert_eq!(l.current(), None);
        // 归还后其它协议可入
        assert_eq!(l.acquire_exclusive(RadioKind::Ble), Ok(()));
    }

    #[test]
    fn wrong_kind_release_is_noop() {
        let mut l = RadioLeases::new();
        l.acquire_exclusive(RadioKind::Ble).unwrap();
        l.release_exclusive(RadioKind::Wifi);
        assert_eq!(l.current(), Some(RadioKind::Ble));
    }

    #[test]
    fn promiscuous_unsupported() {
        let mut l = RadioLeases::new();
        assert_eq!(
            l.acquire_promiscuous(PromKind::Spectrum),
            Err(LeaseError::Unsupported)
        );
    }
}
