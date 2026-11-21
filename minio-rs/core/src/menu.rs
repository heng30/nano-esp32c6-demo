//! 菜单树与导航：上游 `menus.h` 的 `menu_idx_t` / `menu_t` / 菜单状态等价物。
//! 一个功能一个 `MenuId` 变体，对照 docs/PORTING.md 的功能清单，保证不遗漏。

/// 菜单项标识，对应上游 `menu_idx_t`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuId {
    // 根层
    Applications,
    Settings,
    About,
    // Applications 子层
    Wifi,
    Bluetooth,
    Spectrum,
    Sdr,
    ZigbeeThread,
    Surveillance,
    Gps,
    FileManager,
    // WiFi 子层
    WifiWardriving,
    WifiSniffer,
    WifiDeauther,
    WifiCatDos,
    WifiSsidSpam,
    WifiCaptivePortal,
    WifiDroneId,
    // Bluetooth 子层
    BleSpammer,
    BleHid,
    BleAdvSniffer,
    BleTrackers,
    // Zigbee/Thread 子层
    Sniffer154,
    Spoof154,
    ZigbeeLight,
    ZigbeeSwitch,
    ThreadBroadcast,
    Sniffer6LoWpan,
    // Settings 子层
    SetDisplay,
    SetMenuStyle,
    SetLogs,
    SetSdCard,
    SetWifi,
    SetStealth,
    SetLightSleep,
    // About 子层
    AboutVersion,
    AboutCredits,
    AboutOta,
}

/// 菜单项定义，对应上游 `menu_t`（省略 last_selected/on_enter 回调，由 app 分发承担）
pub struct Menu {
    /// 唯一标识
    pub id: MenuId,
    /// 父菜单；根层为 None
    pub parent: Option<MenuId>,
    /// OLED/日志显示名
    pub name: &'static str,
}

/// 全量菜单树；同层子项的显示顺序即数组顺序
pub const MENUS: &[Menu] = &[
    // 根层
    Menu { id: MenuId::Applications, parent: None, name: "Applications" },
    Menu { id: MenuId::Settings, parent: None, name: "Settings" },
    Menu { id: MenuId::About, parent: None, name: "About" },
    // Applications 子层
    Menu { id: MenuId::Wifi, parent: Some(MenuId::Applications), name: "WiFi" },
    Menu { id: MenuId::Bluetooth, parent: Some(MenuId::Applications), name: "Bluetooth" },
    Menu { id: MenuId::Spectrum, parent: Some(MenuId::Applications), name: "Spectrum" },
    Menu { id: MenuId::Sdr, parent: Some(MenuId::Applications), name: "SDR" },
    Menu { id: MenuId::ZigbeeThread, parent: Some(MenuId::Applications), name: "Zigbee/Thread" },
    Menu { id: MenuId::Surveillance, parent: Some(MenuId::Applications), name: "Surveillance" },
    Menu { id: MenuId::Gps, parent: Some(MenuId::Applications), name: "GPS" },
    Menu { id: MenuId::FileManager, parent: Some(MenuId::Applications), name: "File Manager" },
    // WiFi 子层
    Menu { id: MenuId::WifiWardriving, parent: Some(MenuId::Wifi), name: "Wardriving" },
    Menu { id: MenuId::WifiSniffer, parent: Some(MenuId::Wifi), name: "WiFi Sniffer" },
    Menu { id: MenuId::WifiDeauther, parent: Some(MenuId::Wifi), name: "WiFi Deauther" },
    Menu { id: MenuId::WifiCatDos, parent: Some(MenuId::Wifi), name: "CatDoS" },
    Menu { id: MenuId::WifiSsidSpam, parent: Some(MenuId::Wifi), name: "SSID Spam" },
    Menu { id: MenuId::WifiCaptivePortal, parent: Some(MenuId::Wifi), name: "Captive Portal" },
    Menu { id: MenuId::WifiDroneId, parent: Some(MenuId::Wifi), name: "DroneID" },
    // Bluetooth 子层
    Menu { id: MenuId::BleSpammer, parent: Some(MenuId::Bluetooth), name: "BLE Spammer" },
    Menu { id: MenuId::BleHid, parent: Some(MenuId::Bluetooth), name: "BLE HID" },
    Menu { id: MenuId::BleAdvSniffer, parent: Some(MenuId::Bluetooth), name: "BLE ADV Sniffer" },
    Menu { id: MenuId::BleTrackers, parent: Some(MenuId::Bluetooth), name: "BLE Trackers" },
    // Zigbee/Thread 子层
    Menu { id: MenuId::Sniffer154, parent: Some(MenuId::ZigbeeThread), name: "802.15.4 Sniffer" },
    Menu { id: MenuId::Spoof154, parent: Some(MenuId::ZigbeeThread), name: "802.15.4 Spoof" },
    Menu { id: MenuId::ZigbeeLight, parent: Some(MenuId::ZigbeeThread), name: "Zigbee Light" },
    Menu { id: MenuId::ZigbeeSwitch, parent: Some(MenuId::ZigbeeThread), name: "Zigbee Switch" },
    Menu { id: MenuId::ThreadBroadcast, parent: Some(MenuId::ZigbeeThread), name: "Thread Broadcast" },
    Menu { id: MenuId::Sniffer6LoWpan, parent: Some(MenuId::ZigbeeThread), name: "6LoWPAN Sniffer" },
    // Settings 子层
    Menu { id: MenuId::SetDisplay, parent: Some(MenuId::Settings), name: "Display" },
    Menu { id: MenuId::SetMenuStyle, parent: Some(MenuId::Settings), name: "Menu Style" },
    Menu { id: MenuId::SetLogs, parent: Some(MenuId::Settings), name: "Logs Output" },
    Menu { id: MenuId::SetSdCard, parent: Some(MenuId::Settings), name: "SD Card" },
    Menu { id: MenuId::SetWifi, parent: Some(MenuId::Settings), name: "WiFi Settings" },
    Menu { id: MenuId::SetStealth, parent: Some(MenuId::Settings), name: "Stealth Mode" },
    Menu { id: MenuId::SetLightSleep, parent: Some(MenuId::Settings), name: "Light Sleep" },
    // About 子层
    Menu { id: MenuId::AboutVersion, parent: Some(MenuId::About), name: "Version & License" },
    Menu { id: MenuId::AboutCredits, parent: Some(MenuId::About), name: "Credits" },
    Menu { id: MenuId::AboutOta, parent: Some(MenuId::About), name: "OTA Update" },
];

/// 按 id 查显示名；未注册的 id 打占位符而不是 panic
pub fn name(id: MenuId) -> &'static str {
    MENUS
        .iter()
        .find(|m| m.id == id)
        .map(|m| m.name)
        .unwrap_or("<unknown>")
}

/// 某层的全部子项
pub fn children(parent: Option<MenuId>) -> impl Iterator<Item = &'static Menu> {
    MENUS.iter().filter(move |m| m.parent == parent)
}

/// 某层子项数量
pub fn child_count(parent: Option<MenuId>) -> usize {
    children(parent).count()
}

/// 树完整性自检：父节点存在、id 无重复；返回条目数
pub fn verify_tree() -> Result<usize, &'static str> {
    for m in MENUS {
        if let Some(p) = m.parent {
            if !MENUS.iter().any(|x| x.id == p) {
                return Err("menu has missing parent");
            }
        }
    }
    for m in MENUS {
        if MENUS.iter().filter(|x| x.id == m.id).count() != 1 {
            return Err("menu has duplicate id");
        }
    }
    Ok(MENUS.len())
}

/// 导航动作结果
pub enum NavAction {
    /// 钻入子层
    Drill,
    /// 启动叶子 app
    App(MenuId),
    /// 无动作
    Nothing,
}

/// 导航状态：当前所在层 + 光标位置
pub struct MenuState {
    /// 当前列表的父 id；根层为 None
    parent: Option<MenuId>,
    /// 光标在该层的下标
    pos: usize,
}

impl Default for MenuState {
    fn default() -> Self {
        Self { parent: None, pos: 0 }
    }
}

impl MenuState {
    /// 当前层的父 id（日志标题用）
    pub fn parent(&self) -> Option<MenuId> {
        self.parent
    }

    /// 光标下标（渲染用）
    pub fn pos_of(&self) -> usize {
        self.pos
    }

    /// 光标当前项
    pub fn selected(&self) -> &'static Menu {
        let n = child_count(self.parent);
        children(self.parent)
            .nth(self.pos % n.max(1))
            .unwrap_or(&MENUS[0])
    }

    /// 短按：下一项，循环滚动
    pub fn next(&mut self) {
        let n = child_count(self.parent);
        if n > 0 {
            self.pos = (self.pos + 1) % n;
        }
    }

    /// 长按：进入光标项——有子层则钻入，叶子返回 App
    pub fn enter(&mut self) -> NavAction {
        let sel = self.selected();
        if child_count(Some(sel.id)) > 0 {
            self.parent = Some(sel.id);
            self.pos = 0;
            NavAction::Drill
        } else {
            NavAction::App(sel.id)
        }
    }

    /// 返回上级；已在根层返回 false
    pub fn back(&mut self) -> bool {
        match self.parent {
            None => false,
            Some(p) => {
                let me = MENUS
                    .iter()
                    .find(|m| m.id == p)
                    .expect("tree verified at boot");
                self.parent = me.parent;
                self.pos = 0;
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_verifies() {
        assert_eq!(verify_tree().unwrap(), MENUS.len());
    }

    #[test]
    fn root_has_three_categories() {
        assert_eq!(child_count(None), 3);
    }

    #[test]
    fn porting_inventory_present() {
        // 对照 docs/PORTING.md：每个清单功能都必须在树上有一个叶子
        let inventory = [
            MenuId::WifiWardriving,
            MenuId::WifiSniffer,
            MenuId::WifiDeauther,
            MenuId::WifiCatDos,
            MenuId::WifiSsidSpam,
            MenuId::WifiCaptivePortal,
            MenuId::WifiDroneId,
            MenuId::BleSpammer,
            MenuId::BleHid,
            MenuId::BleAdvSniffer,
            MenuId::BleTrackers,
            MenuId::Spectrum,
            MenuId::Sdr,
            MenuId::Sniffer154,
            MenuId::Spoof154,
            MenuId::ZigbeeLight,
            MenuId::ZigbeeSwitch,
            MenuId::ThreadBroadcast,
            MenuId::Sniffer6LoWpan,
            MenuId::Surveillance,
            MenuId::Gps,
            MenuId::FileManager,
            MenuId::SetDisplay,
            MenuId::SetMenuStyle,
            MenuId::SetLogs,
            MenuId::SetSdCard,
            MenuId::SetWifi,
            MenuId::SetStealth,
            MenuId::SetLightSleep,
            MenuId::AboutVersion,
            MenuId::AboutCredits,
            MenuId::AboutOta,
        ];
        for id in inventory {
            assert!(MENUS.iter().any(|m| m.id == id), "{:?} missing", id);
        }
    }

    #[test]
    fn next_wraps_around() {
        let mut s = MenuState::default();
        for _ in 0..3 {
            s.next();
        }
        assert_eq!(s.selected().id, MenuId::Applications);
    }

    #[test]
    fn drill_enter_and_back() {
        let mut s = MenuState::default();
        // 根层光标 0 = Applications，钻入
        assert!(matches!(s.enter(), NavAction::Drill));
        assert_eq!(s.parent(), Some(MenuId::Applications));
        // Applications 第一个子项是 Wifi 分类，再钻入
        assert!(matches!(s.enter(), NavAction::Drill));
        assert_eq!(s.parent(), Some(MenuId::Wifi));
        // Wifi 第一个子项 Wardriving 是叶子
        assert!(matches!(s.enter(), NavAction::App(MenuId::WifiWardriving)));
        // 逐级退回根层
        assert!(s.back());
        assert_eq!(s.parent(), Some(MenuId::Applications));
        assert!(s.back());
        assert_eq!(s.parent(), None);
        assert!(!s.back());
    }
}
