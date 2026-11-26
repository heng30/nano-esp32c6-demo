//! 自动演示：每一拍按当前界面状态挑一个键，没人操作时自己走遍所有页面。

use kk_ui::{KEY_DOWN, KEY_OK, KkUi, PageRoute};

use super::tables::{HOME_ITEMS, ROUTE_COMPONENTS, ROUTE_HOME};

/// 自动演示记录「哪些行打开过」用的页面槽位数，按页面编号取模。
const AUTOPLAY_PAGES: usize = 8;

/// 自动演示：每一拍按当前界面状态挑一个键，没人操作时自己走遍所有页面。
///
/// 刻意不用固定按键脚本：脚本会跟着界面状态漂移（某个条目被禁用后同样的按键
/// 落到别的条目上），最后卡在同一个弹窗里打转。这里只看「弹窗开着吗 / 当前哪一页 /
/// 选中第几行」，所以怎么漂都能自己走出来。
pub struct Autoplay {
    /// 下一个要进的首页图标下标。
    target: u16,
    /// 每个菜单页本轮已经打开过的行（按行号记位）。
    opened: [u32; AUTOPLAY_PAGES],
}

impl Default for Autoplay {
    fn default() -> Self {
        Self::new()
    }
}

impl Autoplay {
    /// 从第一步开始。
    pub const fn new() -> Self {
        Self {
            target: 0,
            opened: [0; AUTOPLAY_PAGES],
        }
    }

    /// 决定这一拍按哪个键。
    pub fn next_key(&mut self, ui: &KkUi<'_>) -> u8 {
        // 弹窗一律用「确定」收尾：整数/开关编辑器两下确定提交并关闭，确认框与消息框一下就关。
        if ui.dialog_open() {
            return KEY_OK;
        }
        let Some(page) = ui.current_page() else {
            return KEY_OK;
        };
        if core::ptr::eq(page, &ROUTE_HOME) {
            // 首页：先把选中项走到目标图标，再按「确定」进去；目标按顺序轮换。
            if ui.selected_index() == self.target {
                self.target = (self.target + 1) % HOME_ITEMS.len() as u16;
                return KEY_OK;
            }
            return KEY_DOWN;
        }
        let items = menu_item_count(page);
        if items == 0 {
            // 信息页与自绘页都用「确定」返回。
            return KEY_OK;
        }
        // 菜单：每一行先按「确定」打开一次，打开过就往下走；走到最后一行（虚拟返回项）
        // 就清空标记并离开，下一轮进来又是逐个打开。
        let selected = ui.selected_index();
        let slot = menu_slot(page);
        if selected < items {
            let bit = 1u32 << (selected & 31);
            if self.opened[slot] & bit == 0 {
                self.opened[slot] |= bit;
                return KEY_OK;
            }
            return KEY_DOWN;
        }
        self.opened[slot] = 0;
        KEY_OK
    }
}

/// 菜单页的条目数（不含虚拟返回项）；非菜单页返回 0。
fn menu_item_count(page: &'static PageRoute) -> u16 {
    match page {
        PageRoute::Menu(menu) => menu.items.len() as u16,
        _ => 0,
    }
}

/// 菜单页在 autoplay 记忆表里的槽位。
fn menu_slot(page: &'static PageRoute) -> usize {
    if core::ptr::eq(page, &ROUTE_COMPONENTS) {
        1
    } else {
        0
    }
}
