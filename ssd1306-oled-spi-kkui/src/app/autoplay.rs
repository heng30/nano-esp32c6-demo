//! 自动演示：每一拍按当前界面状态挑一个键，没人操作时自己走遍所有页面。

use kk_ui::{KEY_DOWN, KEY_OK, KkUi, PageRoute};

use super::tables::{
    BITS_CHANNELS, HOME_ITEMS, LINE_INPUT_DEVICE, ROUTE_COMPONENTS, ROUTE_CONTROLS, ROUTE_HOME,
};

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
    /// 位掩码弹窗里下一步按方向键还是确定。
    bits_move: bool,
    /// 文本编辑器演示已经走到第几步。
    line_input_step: u8,
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
            bits_move: true,
            line_input_step: 0,
        }
    }

    /// 决定这一拍按哪个键。
    pub fn next_key(&mut self, ui: &KkUi<'_>) -> (u8, i16) {
        // 位掩码弹窗是一列方片，只按确定会把第 0 位来回翻、永远走不到按钮，
        // 所以在这里按「方向键走位、确定翻转」交替操作，走到末位后确定会提交。
        if ui.get_bits_editor_draft(&BITS_CHANNELS).is_some() {
            self.bits_move = !self.bits_move;
            return (if self.bits_move { KEY_DOWN } else { KEY_OK }, 0);
        }
        // 文本编辑器不按确定收尾：确定只输入光标下的字符，得先打几个字再走到底行的
        // 「OK」。行内移动太慢，这里用编码器一步滑到右端的换行按钮。
        if !ui.dialog_open() {
            self.line_input_step = 0;
        }
        if ui.line_input_open(&LINE_INPUT_DEVICE) {
            let step = self.line_input_step;
            self.line_input_step = step.wrapping_add(1);
            return match step {
                // 先在字母区打两个字，再滑到第 0 行右端的换行按钮。
                0 | 1 => (KEY_OK, 0),
                2 => (0, 10),
                // 连续踩三下换行按钮下到功能行，退回一格踩确定提交。
                3..=5 => (KEY_OK, 0),
                6 => (0, -1),
                _ => (KEY_OK, 0),
            };
        }
        // 其它弹窗一律用「确定」收尾：整数/开关/枚举编辑器两下确定提交并关闭，
        // 确认框与消息框一下就关。
        if ui.dialog_open() {
            return (KEY_OK, 0);
        }
        let Some(page) = ui.current_page() else {
            return (KEY_OK, 0);
        };
        if core::ptr::eq(page, &ROUTE_HOME) {
            // 首页：先把选中项走到目标图标，再按「确定」进去；目标按顺序轮换。
            if ui.selected_index() == self.target {
                self.target = (self.target + 1) % HOME_ITEMS.len() as u16;
                return (KEY_OK, 0);
            }
            return (KEY_DOWN, 0);
        }
        let items = menu_item_count(page);
        if items == 0 {
            // 信息页与自绘页都用「确定」返回。
            return (KEY_OK, 0);
        }
        // 菜单：每一行先按「确定」打开一次，打开过就往下走；走到最后一行（虚拟返回项）
        // 就清空标记并离开，下一轮进来又是逐个打开。
        let selected = ui.selected_index();
        let slot = menu_slot(page);
        if selected < items {
            let bit = 1u32 << (selected & 31);
            if self.opened[slot] & bit == 0 {
                self.opened[slot] |= bit;
                return (KEY_OK, 0);
            }
            return (KEY_DOWN, 0);
        }
        self.opened[slot] = 0;
        (KEY_OK, 0)
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
    } else if core::ptr::eq(page, &ROUTE_CONTROLS) {
        2
    } else {
        0
    }
}
