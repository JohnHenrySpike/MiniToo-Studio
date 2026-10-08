//! Palette tokens (§16.2) and the egui style derived from them.

use egui::Color32;
use std::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub shell: Color32,
    pub shell_hi: Color32,
    pub shell_lo: Color32,
    pub well: Color32,
    pub well_line: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_disabled: Color32,
    pub key: Color32,
    pub key_hi: Color32,
    pub key_edge: Color32,
    pub key_text: Color32,
    pub accent: Color32,
    pub accent_hi: Color32,
    pub accent_edge: Color32,
    pub accent_text: Color32,
    pub plate: Color32,
    pub plate_text: Color32,
    pub hover: Color32,
    pub selection: Color32,
    pub screen: Color32,
    pub bezel: Color32,
    pub bezel_lo: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub info: Color32,
    pub body: Color32,
    pub body_hi: Color32,
    pub body_lo: Color32,
    pub body_edge: Color32,
    /// editor and capture canvas
    pub canvas: Color32,
}

pub const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// `#AARRGGBB`
pub const fn argb(v: u32) -> Color32 {
    Color32::from_rgba_unmultiplied_const((v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8)
}

pub const WHITE: Color32 = Color32::WHITE;

impl Palette {
    pub const BEIGE: Palette = Palette {
        dark: false,
        bg: hex(0xc9bfad),
        shell: hex(0xe8dfd0),
        shell_hi: hex(0xf7f2e9),
        shell_lo: hex(0xb2a690),
        well: hex(0xddd3c2),
        well_line: hex(0xb3a790),
        text: hex(0x38312a),
        text_dim: hex(0x776c60),
        text_disabled: hex(0xa3988a),
        key: hex(0xd9d4cb),
        key_hi: hex(0xe7e3dc),
        key_edge: hex(0xa29b90),
        key_text: hex(0x3d3832),
        accent: hex(0xee6b3d),
        accent_hi: hex(0xf7835a),
        accent_edge: hex(0xa84322),
        accent_text: hex(0xc24c25),
        plate: hex(0x47433f),
        plate_text: hex(0xefe7d8),
        hover: argb(0x12000000),
        selection: argb(0x40ee6b3d),
        screen: hex(0x0b0c0f),
        bezel: hex(0xd5c9b4),
        bezel_lo: hex(0xb4a790),
        ok: hex(0x2f9a55),
        warn: hex(0xc98110),
        danger: hex(0xd33a2c),
        info: hex(0x3a78c0),
        body: hex(0xe9e0d1),
        body_hi: hex(0xf6f1e8),
        body_lo: hex(0xc2b6a1),
        body_edge: hex(0xa99c86),
        canvas: hex(0x2a2724),
    };

    pub const DARK: Palette = Palette {
        dark: true,
        bg: hex(0x1b1916),
        shell: hex(0x2a2622),
        shell_hi: hex(0x36312b),
        shell_lo: hex(0x0f0d0b),
        well: hex(0x211e1b),
        well_line: hex(0x0e0c0a),
        text: hex(0xece4d6),
        text_dim: hex(0xa1968a),
        text_disabled: hex(0x6d655c),
        key: hex(0x433e38),
        key_hi: hex(0x4f4943),
        key_edge: hex(0x181614),
        key_text: hex(0xece4d6),
        accent: hex(0xee6b3d),
        accent_hi: hex(0xf7835a),
        accent_edge: hex(0xa84322),
        accent_text: hex(0xff8b5e),
        plate: hex(0xd8cebd),
        plate_text: hex(0x2f2a25),
        hover: argb(0x16ffffff),
        selection: argb(0x40ee6b3d),
        screen: hex(0x0b0c0f),
        bezel: hex(0x3b3631),
        bezel_lo: hex(0x1d1a17),
        ok: hex(0x4cc27a),
        warn: hex(0xe8a43a),
        danger: hex(0xf0604f),
        info: hex(0x6aa6ea),
        body: hex(0xe9e0d1),
        body_hi: hex(0xf6f1e8),
        body_lo: hex(0xc2b6a1),
        body_edge: hex(0xa99c86),
        canvas: hex(0x141210),
    };

    pub fn of(theme: crate::api::Theme) -> Palette {
        match theme {
            crate::api::Theme::Beige => Palette::BEIGE,
            crate::api::Theme::Dark => Palette::DARK,
        }
    }
}

thread_local! {
    static CURRENT: Cell<Palette> = const { Cell::new(Palette::BEIGE) };
}

/// The palette of the frame being drawn.
pub fn pal() -> Palette {
    CURRENT.with(|c| c.get())
}

pub fn set_pal(p: Palette) {
    CURRENT.with(|c| c.set(p));
}

/// Qt's `QColor::lighter(factor)` for opaque colours (HSV value scaled).
pub fn lighter(c: Color32, factor: f32) -> Color32 {
    let col = crate::color::Color::rgba(c.r(), c.g(), c.b(), c.a()).lighter(factor);
    Color32::from_rgba_unmultiplied(col.r, col.g, col.b, col.a)
}

pub fn darker(c: Color32, factor: f32) -> Color32 {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    let col = crate::color::Color::rgba(r, g, b, a).darker(factor);
    Color32::from_rgba_unmultiplied(col.r, col.g, col.b, col.a)
}

/// Multiplies the alpha (straight alpha semantics).
pub fn fade(c: Color32, opacity: f32) -> Color32 {
    c.gamma_multiply(opacity.clamp(0.0, 1.0))
}

/// The egui style for a palette: only what built-in widgets (text edits, scroll bars,
/// tooltips) pick up — everything else is painted by our own widgets.
pub fn apply_style(ctx: &egui::Context, p: &Palette) {
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        v.dark_mode = p.dark;
        v.override_text_color = None;
        v.panel_fill = p.bg;
        v.window_fill = p.plate;
        v.window_stroke = egui::Stroke::NONE;
        v.window_corner_radius = egui::CornerRadius::same(4);
        v.menu_corner_radius = egui::CornerRadius::same(4);
        v.popup_shadow = egui::Shadow { offset: [0, 2], blur: 6, spread: 0, color: argb(0x30000000) };
        v.window_shadow = v.popup_shadow;
        v.extreme_bg_color = p.well;
        v.faint_bg_color = p.shell;
        v.code_bg_color = p.well;
        v.hyperlink_color = p.accent_text;
        v.selection.bg_fill = p.accent;
        v.selection.stroke = egui::Stroke::new(1.0, WHITE);
        v.text_cursor.stroke = egui::Stroke::new(1.5, p.text);
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.bg_stroke = egui::Stroke::NONE;
            w.corner_radius = egui::CornerRadius::same(3);
            w.expansion = 0.0;
        }
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, p.text);
        v.widgets.noninteractive.bg_fill = p.shell;
        v.widgets.noninteractive.weak_bg_fill = p.shell;
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, p.text);
        v.widgets.inactive.bg_fill = fade(p.text_dim, 0.5);
        v.widgets.inactive.weak_bg_fill = p.key;
        v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, p.text);
        v.widgets.hovered.bg_fill = fade(p.text_dim, 0.8);
        v.widgets.hovered.weak_bg_fill = p.key_hi;
        v.widgets.active.fg_stroke = egui::Stroke::new(1.0, p.text);
        v.widgets.active.bg_fill = p.accent;
        v.widgets.active.weak_bg_fill = p.key_hi;
        v.widgets.open = v.widgets.active;

        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(8.0, 6.0);
        s.button_padding = egui::vec2(10.0, 6.0);
        s.interact_size = egui::vec2(20.0, 20.0);
        s.scroll = egui::style::ScrollStyle {
            floating: true,
            bar_width: 7.0,
            floating_allocated_width: 0.0,
            handle_min_length: 24.0,
            bar_inner_margin: 2.0,
            bar_outer_margin: 2.0,
            dormant_background_opacity: 0.0,
            active_background_opacity: 0.0,
            interact_background_opacity: 0.0,
            dormant_handle_opacity: 0.0,
            active_handle_opacity: 0.5,
            interact_handle_opacity: 0.8,
            content_margin: egui::Margin::ZERO,
            fade: egui::style::ScrollFadeStyle { strength: 0.0, size: 0.0 },
            ..egui::style::ScrollStyle::floating()
        };
        style.interaction.tooltip_delay = 0.45;
        style.interaction.show_tooltips_only_when_still = false;
        style.interaction.selectable_labels = false;
        style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(13.0));
        style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
        style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(11.0));
        style.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(12.0));
    });
}
