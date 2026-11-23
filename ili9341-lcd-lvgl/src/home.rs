//! 第二页：客厅控制面板——左侧恒温器圆弧，右侧灯光开关、亮度滑条、场景按钮。
//!
//! 纯展示页，值都写死；什么时候显示由 `main` 的帧循环按时间决定。

use lvgl_rust::{
    Error, Font, Handle, Lvgl, Obj, Owned, Selector, WidgetExt,
    color::hex,
    enums::{Align, Part, State},
    sys,
    widgets::{Arc, Button, Label, Slider, Switch},
};

/// 屏幕底色。
const BG: u32 = 0x0b0d10;
/// 卡片底色。
const CARD: u32 = 0x161a1e;
/// 轨道与未激活元素。
const TRACK: u32 = 0x2a2f36;
/// 高亮蓝（圆弧、开关、滑条、按钮）。
const ACCENT: u32 = 0x2196f3;
/// 主文字。
const TEXT: u32 = 0xffffff;
/// 次要文字。
const MUTED: u32 = 0x7a8794;

/// 卡片（x, y, w, h）：屏 320×240 四周留 6 px。
const CARD_BOX: (i32, i32, i32, i32) = (6, 6, 308, 228);
/// 温控圆弧的外框（左侧那份表盘）。
const ARC_BOX: (i32, i32, i32, i32) = (20, 70, 120, 120);
/// 圆弧线宽。
const ARC_WIDTH: i32 = 10;
/// 圆弧行程百分比。
const ARC_VALUE: i32 = 62;
/// 右侧控件列的左边与宽度。
const COL_X: i32 = 170;
const COL_W: i32 = 132;
/// 右侧各行的 y。
const LIGHTS_Y: i32 = 66;
const BRIGHT_Y: i32 = 108;
const SLIDER_Y: i32 = 140;
const BUTTON_Y: i32 = 164;
/// 开关尺寸。
const SWITCH_W: i32 = 42;
const SWITCH_H: i32 = 22;
/// 滑条高度。
const SLIDER_H: i32 = 8;
/// 按钮高度。
const BUTTON_H: i32 = 34;

/// 客厅控制页。
pub struct Home {
    /// 这页自己的屏幕：没有父对象，必须由这里持有，否则 drop 时连屏幕一起删掉。
    _owned: Owned<Obj>,
    /// 同一个屏幕的句柄，切页时交给 `Lvgl::load_screen`。
    pub screen: Handle<Obj>,
}

impl Home {
    /// 建一块新屏幕，把整页控件摆上去。
    pub fn build(lv: &mut Lvgl) -> Result<Self, Error> {
        let owned = lv.create_screen()?;
        let screen = owned.get(lv)?.handle()?;

        let (card, title, clock, arc, temp, heating, lights, sw, bright, slider, button) = {
            let mut scr = owned.get_mut(lv)?;

            let card = Obj::create(&mut scr)?.handle()?;
            let title = Label::create(&mut scr)?.handle()?;
            let clock = Label::create(&mut scr)?.handle()?;
            let arc = Arc::create(&mut scr)?.handle()?;
            let temp = Label::create(&mut scr)?.handle()?;
            let heating = Label::create(&mut scr)?.handle()?;
            let lights = Label::create(&mut scr)?.handle()?;
            let sw = Switch::create(&mut scr)?.handle()?;
            let bright = Label::create(&mut scr)?.handle()?;
            let slider = Slider::create(&mut scr)?.handle()?;
            let button = Button::create(&mut scr)?.handle()?;

            (
                card, title, clock, arc, temp, heating, lights, sw, bright, slider, button,
            )
        };

        {
            let mut o = screen.get_mut(lv)?;
            o.set_style_bg_color(hex(BG), Selector::DEFAULT);
            o.set_style_bg_opa(255, Selector::DEFAULT);
        }

        // 卡片：圆角深灰板，所有控件都摆在它上面。
        let (cx, cy, cw, ch) = CARD_BOX;
        {
            let mut o = card.get_mut(lv)?;
            o.set_pos(cx, cy);
            o.set_size(cw, ch);
            o.set_style_bg_color(hex(CARD), Selector::DEFAULT);
            o.set_style_bg_opa(255, Selector::DEFAULT);
            o.set_style_border_width(1, Selector::DEFAULT);
            o.set_style_border_color(hex(0x23282e), Selector::DEFAULT);
            o.set_style_radius(10, Selector::DEFAULT);
            o.set_style_pad_all(0, Selector::DEFAULT);
        }

        // 顶部一行：左「Living room」，右时钟。
        {
            let mut o = title.get_mut(lv)?;
            o.view().set_text_static(Some(c"Living room"))?;
            o.set_pos(cx + 18, cy + 10);
            o.set_style_text_color(hex(TEXT), Selector::DEFAULT);
        }
        {
            let mut o = clock.get_mut(lv)?;
            o.view().set_text_static(Some(c"21:42"))?;
            o.set_align(Align::TOP_RIGHT);
            o.set_style_text_color(hex(MUTED), Selector::DEFAULT);
            o.set_style_translate_x(-18, Selector::DEFAULT);
            o.set_style_translate_y(10, Selector::DEFAULT);
        }

        // 恒温器圆弧：底轨一道深灰，行程一段亮蓝，末端一个圆点旋钮。
        {
            let mut o = arc.get_mut(lv)?;
            o.set_pos(ARC_BOX.0, ARC_BOX.1);
            o.set_size(ARC_BOX.2, ARC_BOX.3);
            o.view().set_bg_angles(135, 405);
            o.view().set_range(0, 100);
            o.view().set_value(ARC_VALUE);
            o.set_style_arc_width(ARC_WIDTH, Selector::DEFAULT);
            o.set_style_arc_width(ARC_WIDTH, Part::INDICATOR);
            o.set_style_arc_color(hex(TRACK), Selector::DEFAULT);
            o.set_style_arc_color(hex(ACCENT), Part::INDICATOR);
            o.set_style_arc_rounded(true, Selector::DEFAULT);
            o.set_style_arc_rounded(true, Part::INDICATOR);
            // 旋钮：默认是完全透明的，得自己给底色和圆形半径
            o.set_style_bg_opa(255, Part::KNOB);
            o.set_style_bg_color(hex(ACCENT), Part::KNOB);
            o.set_style_radius(sys::LV_RADIUS_CIRCLE as i32, Part::KNOB);
            o.set_style_pad_all(ARC_WIDTH / 2, Part::KNOB);
        }

        // 圆弧里的温度读数与状态。
        let center = (ARC_BOX.0 + ARC_BOX.2 / 2, ARC_BOX.1 + ARC_BOX.3 / 2);
        {
            let mut o = temp.get_mut(lv)?;
            o.view().set_text_static(Some(c"21.5\xc2\xb0"))?;
            o.set_style_text_font(Font::montserrat_28(), Selector::DEFAULT);
            o.set_style_text_color(hex(TEXT), Selector::DEFAULT);
            o.set_pos(center.0 - 34, center.1 - 22);
        }
        {
            let mut o = heating.get_mut(lv)?;
            o.view().set_text_static(Some(c"Heating"))?;
            o.set_style_text_color(hex(MUTED), Selector::DEFAULT);
            o.set_pos(center.0 - 24, center.1 + 12);
        }

        // 右侧：灯光开关。
        {
            let mut o = lights.get_mut(lv)?;
            o.view().set_text_static(Some(c"Lights"))?;
            o.set_pos(COL_X, LIGHTS_Y + 4);
            o.set_style_text_color(hex(TEXT), Selector::DEFAULT);
        }
        {
            let mut o = sw.get_mut(lv)?;
            o.set_pos(COL_X + COL_W - SWITCH_W, LIGHTS_Y);
            o.set_size(SWITCH_W, SWITCH_H);
            o.add_state(State::CHECKED);
            o.set_style_bg_color(hex(TRACK), Selector::DEFAULT);
            o.set_style_bg_opa(255, Selector::DEFAULT);
            o.set_style_bg_color(hex(ACCENT), Part::INDICATOR);
            o.set_style_bg_opa(255, Part::INDICATOR);
            o.set_style_bg_color(hex(TEXT), Part::KNOB);
            o.set_style_bg_opa(255, Part::KNOB);
        }

        // 右侧：亮度标题 + 滑条。
        {
            let mut o = bright.get_mut(lv)?;
            o.view().set_text_static(Some(c"Brightness 70%"))?;
            o.set_pos(COL_X, BRIGHT_Y);
            o.set_style_text_color(hex(TEXT), Selector::DEFAULT);
        }
        {
            let mut o = slider.get_mut(lv)?;
            o.set_pos(COL_X, SLIDER_Y);
            o.set_size(COL_W, SLIDER_H);
            o.view().set_range(0, 100);
            o.view().set_value(70, false);
            o.set_style_bg_color(hex(TRACK), Selector::DEFAULT);
            o.set_style_bg_opa(255, Selector::DEFAULT);
            o.set_style_bg_color(hex(ACCENT), Part::INDICATOR);
            o.set_style_bg_opa(255, Part::INDICATOR);
            o.set_style_bg_color(hex(ACCENT), Part::KNOB);
            o.set_style_bg_opa(255, Part::KNOB);
            o.set_style_radius(sys::LV_RADIUS_CIRCLE as i32, Part::KNOB);
        }

        // 右侧：场景按钮。
        {
            let mut o = button.get_mut(lv)?;
            o.set_pos(COL_X, BUTTON_Y);
            o.set_size(COL_W, BUTTON_H);
            o.set_style_bg_color(hex(ACCENT), Selector::DEFAULT);
            o.set_style_bg_opa(255, Selector::DEFAULT);
            o.set_style_border_width(0, Selector::DEFAULT);
            o.set_style_radius(8, Selector::DEFAULT);

            let label = Label::create(&mut o)?;
            label.set_text_static(Some(c"Evening scene"))?;
            label.handle()?
        };

        Ok(Self {
            _owned: owned,
            screen,
        })
    }
}
