//! Navigation column (§16.5): logo, six page keys with LEDs, the theme key.

use super::pixel::{self, PixStyle};
use super::theme::{WHITE, hex, pal};
use super::widgets::{self as w, Key};
use crate::api::{DisplayMode, Snapshot, Theme};
use crate::claude::ClaudeState;
use egui::{Rect, Sense, Ui, pos2, vec2};

pub struct NavOut {
    pub page: Option<usize>,
    pub toggle_theme: bool,
}

/// Pages: (title key, subtitle key, icon). Texts go through `tr!` where they are shown.
pub const PAGES: [(&str, &str, &str); 6] = [
    ("nav.image", "nav.image_sub", "image"),
    ("nav.screen", "nav.screen_sub", "screen"),
    ("nav.modes", "nav.modes_sub", "modes"),
    ("nav.claude", "nav.claude_sub", "claude"),
    ("nav.device", "nav.device_sub", "device"),
    ("nav.settings", "nav.settings_sub", "settings"),
];

fn page_mode(i: usize) -> Option<DisplayMode> {
    match i {
        0 => Some(DisplayMode::Image),
        1 => Some(DisplayMode::Screen),
        2 => Some(DisplayMode::Live),
        3 => Some(DisplayMode::Claude),
        _ => None,
    }
}

pub fn show(ui: &mut Ui, snap: &Snapshot, current: usize, narrow: bool, theme: Theme) -> NavOut {
    let p = pal();
    let mut out = NavOut { page: None, toggle_theme: false };
    let full = ui.max_rect();
    let area = Rect::from_min_max(full.min + vec2(14.0, 14.0), full.max - vec2(15.0, 14.0));

    // logo
    let logo_top = area.top() + 4.0;
    let title = PixStyle::new(11).zoom(2);
    let painter = ui.painter().clone();
    let logo_h = (title.height("MiniToo") + 3.0 + 14.0).max(36.0);
    if narrow {
        pixel::paint_icon(&painter, pos2(area.center().x - 18.0, logo_top), "device", 3, p.text, Some(p.accent));
    } else {
        pixel::paint_icon(&painter, pos2(area.left() + 2.0, logo_top), "device", 3, p.text, Some(p.accent));
        let tx = area.left() + 2.0 + 36.0 + 10.0;
        let th = title.height("MiniToo");
        let col_h = th + 3.0 + 14.0;
        let ty = logo_top + logo_h / 2.0 - col_h / 2.0;
        pixel::paint_text(&painter, pos2(tx, ty), "MiniToo", &title, p.text);
        w::paint_plate(&painter, pos2(tx, ty + th + 3.0), "studio", p.plate, p.plate_text);
    }

    let claude_alert = snap.claude.state == ClaudeState::Alerting;
    let mut y = logo_top + if narrow { 36.0 } else { logo_h } + 14.0 + 8.0;
    for (i, (name, sub, icon)) in PAGES.iter().enumerate() {
        let (name, sub) = (tr!(*name), tr!(*sub));
        let rect = Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), 54.0));
        y += 54.0 + 8.0;
        let id = ui.id().with(("nav", i));
        let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let is_cur = i == current;
        let down = resp.is_pointer_button_down_on();
        let face = if is_cur {
            p.accent
        } else if resp.hovered() {
            p.key_hi
        } else {
            p.key
        };
        let edge = if is_cur { p.accent_edge } else { p.key_edge };
        w::paint_plastic(&painter, rect, face, edge, None, 3.0, 8.0, is_cur || down);
        let sink = if is_cur || down { 2.0 } else { 0.0 };
        let content = Rect::from_min_max(
            pos2(rect.left() + if narrow { 4.0 } else { 12.0 }, rect.top() + sink),
            pos2(rect.right() - if narrow { 4.0 } else { 12.0 }, rect.bottom() - 3.0 + sink.min(0.0)),
        );
        let cy = content.center().y;
        let fg = if is_cur { WHITE } else { p.key_text };
        let sec = if is_cur { hex(0xffd2bd) } else { p.accent };
        let active = page_mode(i).is_some_and(|m| m == snap.mode);
        let led_alert = i == 3 && claude_alert;
        let led_color = if led_alert { p.danger } else { p.ok };
        if narrow {
            let total = 24.0 + if active { 3.0 + 10.0 } else { 0.0 };
            let x0 = content.center().x - total / 2.0;
            pixel::paint_icon(&painter, pos2(x0, cy - 12.0), icon, 2, fg, Some(sec));
            if active {
                let lr = Rect::from_min_size(pos2(x0 + 27.0, cy - 5.0), vec2(10.0, 10.0));
                let op = if led_alert { w::blink_opacity(ui, 0.45, 0.35) } else { 1.0 };
                w::paint_led(&painter, lr, led_color, true, op);
            }
        } else {
            pixel::paint_icon(&painter, pos2(content.left(), cy - 12.0), icon, 2, fg, Some(sec));
            let tx = content.left() + 24.0 + 11.0;
            let led_w = if active { 10.0 + 11.0 } else { 0.0 };
            let text_w = content.right() - tx - led_w;
            let st = PixStyle::new(12);
            let th = st.height(name);
            let sub_font = w::font(11.0);
            let sub_color = if is_cur { hex(0xffe6da) } else { p.text_dim };
            let g = w::galley_elided(ui, sub, sub_font, sub_color, text_w);
            let col_h = th + 2.0 + g.size().y;
            let top = cy - col_h / 2.0;
            pixel::paint_text(&painter, pos2(tx, top), &st.elide(name, text_w), &st, fg);
            painter.galley(pos2(tx, top + th + 2.0), g, sub_color);
            if active {
                let lr = Rect::from_min_size(pos2(content.right() - 10.0, cy - 5.0), vec2(10.0, 10.0));
                let op = if led_alert { w::blink_opacity(ui, 0.45, 0.35) } else { 1.0 };
                w::paint_led(&painter, lr, led_color, true, op);
                let lresp = ui.interact(lr.expand(3.0), id.with("led"), Sense::hover());
                w::tip(lresp, tr!("nav.on_device"));
            }
        }
        let resp = if narrow { w::tip(resp, &format!("{name} — {sub}")) } else { resp };
        if resp.clicked() {
            out.page = Some(i);
        }
    }

    // theme key at the bottom right
    let dark = theme == Theme::Dark;
    let key = Key::icon_only(if dark { "sun" } else { "moon" }).flat().tip(if dark { tr!("nav.theme_beige") } else { tr!("nav.theme_dark") });
    let size = key.size(ui);
    let r = Rect::from_min_size(pos2(area.right() - size.x, area.bottom() - size.y), size);
    if key.show_at(ui, r).clicked() {
        out.toggle_theme = true;
    }
    out
}
