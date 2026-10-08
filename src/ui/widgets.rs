//! Primitives (§16.3) and styled controls (§16.4), all painted by hand.

use super::pixel::{self, PixStyle};
use super::theme::{WHITE, darker, fade, lighter, pal};
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Color32, CornerRadius, FontFamily, FontId, Galley, Id, Layout, Margin, Painter, Pos2, Rect, Response, Sense, Shape,
    Stroke, StrokeKind, Ui, UiBuilder, Vec2, pos2, vec2,
};
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------- fonts and text

pub fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub fn cr(r: f32) -> CornerRadius {
    CornerRadius::same(r.round().clamp(0.0, 255.0) as u8)
}

/// Qt renders small hinted text a little tighter than egui; this closes the gap.
pub fn tight(size: f32) -> f32 {
    -(size * 0.04)
}

pub fn job(text: &str, font: FontId, color: Color32, wrap_width: f32, halign: Align) -> LayoutJob {
    let mut job = LayoutJob::default();
    let fmt = egui::TextFormat { extra_letter_spacing: tight(font.size), font_id: font, color, ..Default::default() };
    job.append(text, 0.0, fmt);
    job.wrap.max_width = wrap_width;
    job.halign = halign;
    job
}

pub fn galley(ui: &Ui, text: &str, font: FontId, color: Color32) -> Arc<Galley> {
    ui.fonts_mut(|f| f.layout_job(job(text, font, color, f32::INFINITY, Align::Min)))
}

pub fn galley_wrapped(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    ui.fonts_mut(|f| f.layout_job(job(text, font, color, width.max(10.0), Align::Min)))
}

pub fn galley_elided(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut j = job(text, font, color, f32::INFINITY, Align::Min);
    j.wrap = TextWrapping::truncate_at_width(width.max(1.0));
    ui.fonts_mut(|f| f.layout_job(j))
}

/// Rich text for tooltips and egui labels, with the same tight spacing.
pub fn rich(text: impl Into<String>, size: f32, color: Color32) -> egui::RichText {
    egui::RichText::new(text).size(size).color(color).extra_letter_spacing(tight(size))
}

pub fn text_width(ui: &Ui, text: &str, font: FontId) -> f32 {
    galley(ui, text, font, Color32::WHITE).size().x
}

/// Elides in the middle («/home/…/project»).
pub fn elide_middle(ui: &Ui, text: &str, font: &FontId, width: f32) -> String {
    if text_width(ui, text, font.clone()) <= width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    let make = |keep: usize| -> String {
        let head = keep.div_ceil(2);
        let tail = keep / 2;
        chars[..head].iter().collect::<String>() + "…" + &chars[chars.len() - tail..].iter().collect::<String>()
    };
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if text_width(ui, &make(mid), font.clone()) <= width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    make(lo)
}

/// One line of text (no wrapping).
pub fn text(ui: &mut Ui, s: &str, font: FontId, color: Color32) -> Response {
    let g = galley(ui, s, font, color);
    let (rect, resp) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(rect.min, g, color);
    resp
}

/// One line elided at `width` (or the available width).
pub fn text_elided(ui: &mut Ui, s: &str, font: FontId, color: Color32, width: Option<f32>) -> Response {
    let w = width.unwrap_or_else(|| ui.available_width());
    let g = galley_elided(ui, s, font, color, w);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x.min(w), g.size().y), Sense::hover());
    ui.painter().galley(rect.min, g, color);
    resp
}

/// Wrapped paragraph taking the full available width (or `width`).
pub fn para(ui: &mut Ui, s: &str, font: FontId, color: Color32) -> Response {
    let w = ui.available_width();
    para_w(ui, s, font, color, w, Align::Min)
}

pub fn para_w(ui: &mut Ui, s: &str, font: FontId, color: Color32, width: f32, align: Align) -> Response {
    let g = ui.fonts_mut(|f| f.layout_job(job(s, font, color, width.max(10.0), align)));
    let (rect, resp) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
    let x = match align {
        Align::Min => rect.left(),
        Align::Center => rect.center().x,
        Align::Max => rect.right(),
    };
    ui.painter().galley(pos2(x, rect.top()), g, color);
    resp
}

/// Dim 13 px wrapped explanation.
pub fn hint(ui: &mut Ui, s: &str) -> Response {
    para(ui, s, font(13.0), pal().text_dim)
}

pub fn hint_small(ui: &mut Ui, s: &str, size: f32) -> Response {
    para(ui, s, font(size), pal().text_dim)
}

/// A horizontal row of fixed height with vertically centred items.
pub fn row<R>(ui: &mut Ui, height: f32, spacing: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(vec2(w, height), Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing = vec2(spacing, 0.0);
        ui.set_min_height(height);
        add(ui)
    })
    .inner
}

/// Rest of a row, right to left.
pub fn right<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.with_layout(Layout::right_to_left(Align::Center), add).inner
}

/// A child Ui in an explicit rect.
pub fn child<R>(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope_builder(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)), add).inner
}

pub fn tip(resp: Response, s: &str) -> Response {
    if s.is_empty() {
        return resp;
    }
    let s = s.to_string();
    resp.on_hover_ui(move |ui| {
        ui.set_max_width(340.0);
        ui.label(rich(s, 12.0, pal().plate_text));
    })
}

pub fn tip_disabled(resp: Response, s: &str) -> Response {
    let s2 = s.to_string();
    let resp = tip(resp, s);
    resp.on_disabled_hover_ui(move |ui| {
        ui.set_max_width(340.0);
        ui.label(rich(s2, 12.0, pal().plate_text));
    })
}

fn snap_px(painter: &Painter, v: f32) -> f32 {
    let ppp = painter.pixels_per_point();
    (v * ppp).round() / ppp
}

// ---------------------------------------------------------------------- primitives

/// Plastic: an edge-coloured body with the face on top, `depth` px shorter; pressed moves
/// the face down by `depth − 1`. Returns the face rect.
#[allow(clippy::too_many_arguments)]
pub fn paint_plastic(painter: &Painter, rect: Rect, face: Color32, edge: Color32, highlight: Option<Color32>, depth: f32, radius: f32, pressed: bool) -> Rect {
    painter.rect_filled(rect, cr(radius), edge);
    let dy = if pressed { (depth - 1.0).max(0.0) } else { 0.0 };
    let face_rect = Rect::from_min_size(rect.min + vec2(0.0, dy), vec2(rect.width(), (rect.height() - depth).max(0.0)));
    painter.rect_filled(face_rect, cr(radius), face);
    if !pressed && face.a() > 0 {
        let hl = highlight.unwrap_or_else(|| lighter(face, 1.07));
        let y = snap_px(painter, face_rect.top() + 1.0);
        let r = Rect::from_min_max(pos2(face_rect.left() + radius * 0.6, y), pos2(face_rect.right() - radius * 0.6, y + 1.0));
        painter.rect_filled(r, 0, hl);
    }
    face_rect
}

/// Well: recessed field with a top inner shadow; focused = 2 px accent frame.
pub fn paint_well(painter: &Painter, rect: Rect, radius: f32, focused: bool, fill: Color32, line: Color32) {
    let p = pal();
    painter.rect_filled(rect, cr(radius), fill);
    let bw = if focused { 2.0 } else { 1.0 };
    let sh = Rect::from_min_size(pos2(rect.left() + radius / 2.0, rect.top() + bw), vec2((rect.width() - radius).max(0.0), 2.0));
    painter.rect_filled(sh, 0, Color32::from_black_alpha(if p.dark { 64 } else { 18 }));
    let stroke = if focused { Stroke::new(2.0, p.accent) } else { Stroke::new(1.0, line) };
    painter.rect_stroke(rect, cr(radius), stroke, StrokeKind::Inside);
}

pub fn well(painter: &Painter, rect: Rect, radius: f32, focused: bool) {
    let p = pal();
    paint_well(painter, rect, radius, focused, p.well, p.well_line);
}

const PLATE: PixStyle = PixStyle::new(8).spacing(1);

pub fn plate_size(text: &str) -> Vec2 {
    let t = text.to_uppercase();
    PLATE.measure(&t) + vec2(12.0, 4.0)
}

/// Plate: dark label with spaced pixel capitals. Returns its rect.
pub fn paint_plate(painter: &Painter, pos: Pos2, text: &str, bg: Color32, fg: Color32) -> Rect {
    let t = text.to_uppercase();
    let size = PLATE.measure(&t) + vec2(12.0, 4.0);
    let rect = Rect::from_min_size(pixel::snap(painter, pos), size);
    painter.rect_filled(rect, 3, bg);
    pixel::paint_text(painter, rect.min + vec2(6.0, 2.0), &t, &PLATE, fg);
    rect
}

pub fn plate(ui: &mut Ui, text: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(plate_size(text), Sense::hover());
    let p = pal();
    paint_plate(ui.painter(), rect.min, text, p.plate, p.plate_text);
    resp
}

/// Bezel thickness and outer size of a ScreenFrame.
pub fn screen_size(factor: f32) -> (f32, Vec2) {
    let bezel = (5.0 + 3.0 * factor).round();
    (bezel, vec2((160.0 * factor).round() + 2.0 * bezel, (128.0 * factor).round() + 2.0 * bezel))
}

/// ScreenFrame: bezel + glass; returns the glass rect.
pub fn paint_screen_frame(painter: &Painter, min: Pos2, factor: f32, live: bool, beige: bool) -> Rect {
    let p = pal();
    let (bezel, size) = screen_size(factor);
    let rect = Rect::from_min_size(pixel::snap(painter, min), size);
    let radius = 4.0 + 2.0 * factor;
    painter.rect_filled(rect, cr(radius), if beige { p.body_lo } else { p.bezel });
    painter.rect_stroke(rect, cr(radius), Stroke::new(1.0, if beige { p.body_edge } else { p.bezel_lo }), StrokeKind::Inside);
    painter.rect_filled(rect.shrink(bezel - 2.0), 3, Color32::from_black_alpha(64));
    let glass = Rect::from_min_size(rect.min + Vec2::splat(bezel), vec2((160.0 * factor).round(), (128.0 * factor).round()));
    painter.rect_filled(glass, 0, p.screen);
    if live {
        painter.rect_stroke(rect.expand(3.0), cr(7.0 + 2.0 * factor), Stroke::new(2.0, p.accent), StrokeKind::Inside);
    }
    glass
}

pub fn screen_frame(ui: &mut Ui, factor: f32, live: bool) -> (Response, Rect) {
    let (_, size) = screen_size(factor);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let glass = paint_screen_frame(ui.painter(), rect.min, factor, live, false);
    (resp, glass)
}

pub fn paint_texture(painter: &Painter, rect: Rect, tex: egui::TextureId, tint: Color32) {
    painter.image(tex, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
}

/// Opacity of a blinking element: 450 ms down to 35% and back.
pub fn blink_opacity(ui: &Ui, period_half: f64, low: f32) -> f32 {
    let t = ui.input(|i| i.time);
    ui.ctx().request_repaint_after(Duration::from_millis(40));
    let phase = (t % (2.0 * period_half)) / period_half;
    let k = if phase < 1.0 { phase } else { 2.0 - phase } as f32;
    1.0 - (1.0 - low) * k
}

/// Led: 10×10 square light; lit ones glow, blinking ones fade to 35%.
pub fn paint_led(painter: &Painter, rect: Rect, color: Color32, lit: bool, opacity: f32) {
    let p = pal();
    if lit {
        painter.rect_filled(rect.expand(3.0), 4, fade(color, 0.25));
    }
    let c = if lit { color } else { p.text_disabled };
    painter.rect_filled(rect, 2, fade(c, opacity));
    painter.rect_stroke(rect, 2, Stroke::new(1.0, fade(darker(c, 1.4), opacity)), StrokeKind::Inside);
}

pub fn led(ui: &mut Ui, color: Color32, lit: bool, blink: bool) -> Response {
    led_sized(ui, 10.0, color, lit, blink)
}

pub fn led_sized(ui: &mut Ui, size: f32, color: Color32, lit: bool, blink: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let op = if blink && lit { blink_opacity(ui, 0.45, 0.35) } else { 1.0 };
    paint_led(ui.painter(), rect, color, lit, op);
    resp
}

/// PixelText as a widget.
pub fn pixel_text(ui: &mut Ui, text: &str, st: PixStyle, color: Color32) -> Response {
    let size = vec2(st.measure(text).x, st.height(text));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    pixel::paint_text(ui.painter(), rect.min, text, &st, color);
    resp
}

/// PixelText elided to `max_w`.
pub fn pixel_text_elided(ui: &mut Ui, text: &str, st: PixStyle, color: Color32, max_w: f32) -> Response {
    let s = st.elide(text, max_w);
    pixel_text(ui, &s, st, color)
}

pub fn pixel_icon(ui: &mut Ui, name: &str, zoom: u32, color: Color32, secondary: Option<Color32>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(12.0 * zoom as f32), Sense::hover());
    pixel::paint_icon(ui.painter(), rect.min, name, zoom, color, secondary);
    resp
}

/// Pixel battery with 4 segments (BatteryGauge).
pub fn battery(ui: &mut Ui, level: Option<u8>, zoom: u32) -> Response {
    let p = pal();
    let z = zoom as f32;
    let fill = match level {
        Some(l) if l <= 20 => p.danger,
        Some(l) if l <= 50 => p.warn,
        _ => p.ok,
    };
    let label = level.map(|l| format!("{l}%")).unwrap_or_else(|| "—".into());
    let st = PixStyle::new(12).zoom(if zoom > 2 { 2 } else { 1 });
    let label_size = vec2(st.measure(&label).x, st.height(&label));
    let size = vec2(15.0 * z + 6.0 + label_size.x, (8.0 * z).max(label_size.y));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    let top = pixel::snap(painter, pos2(rect.left(), rect.center().y - 4.0 * z));
    let body = Rect::from_min_size(top, vec2(13.0 * z, 8.0 * z));
    painter.rect_stroke(body, 0, Stroke::new(z, p.text), StrokeKind::Inside);
    for i in 0..4 {
        if level.is_some_and(|l| l as i32 > i * 25) {
            let seg = Rect::from_min_size(top + vec2(2.0 * z + i as f32 * 2.5 * z, 2.0 * z), vec2(1.5 * z, 4.0 * z));
            painter.rect_filled(seg, 0, fill);
        }
    }
    painter.rect_filled(Rect::from_min_size(top + vec2(13.0 * z, 2.0 * z), vec2(2.0 * z, 4.0 * z)), 0, p.text);
    let color = if level.is_some_and(|l| l <= 20) { p.danger } else { p.text };
    pixel::paint_text(painter, pos2(body.right() + 2.0 * z + 6.0, rect.center().y - label_size.y / 2.0), &label, &st, color);
    resp
}

// ---------------------------------------------------------------------- panels

/// Frame / GroupBox: beige plastic panel. `margin` = (left, top, right, bottom).
pub fn panel_ex<R>(ui: &mut Ui, margin: [f32; 4], title: Option<&str>, min_height: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    panel_measured(ui, margin, title, min_height, add).0
}

/// Like [`panel_ex`], also returning the height the content alone would need.
pub fn panel_measured<R>(ui: &mut Ui, margin: [f32; 4], title: Option<&str>, min_height: f32, add: impl FnOnce(&mut Ui) -> R) -> (R, f32) {
    let p = pal();
    let edge_idx = ui.painter().add(Shape::Noop);
    let face_idx = ui.painter().add(Shape::Noop);
    let hl_idx = ui.painter().add(Shape::Noop);
    let outer = ui.available_rect_before_wrap();
    let width = ui.available_width();
    let mut top = margin[1];
    if title.is_some() {
        top += 14.0 + 12.0;
    }
    let inner = Rect::from_min_max(
        pos2(outer.left() + margin[0], outer.top() + top),
        pos2(outer.left() + width - margin[2], outer.bottom().max(outer.top() + top + 10.0) - margin[3]),
    );
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)));
    child.set_width(inner.width());
    let r = add(&mut child);
    let content = child.min_rect();
    let natural = content.bottom() + margin[3] - outer.top();
    let height = natural.max(min_height);
    let rect = Rect::from_min_size(outer.min, vec2(width, height));
    let depth = 3.0;
    let painter = ui.painter();
    painter.set(edge_idx, egui::epaint::RectShape::filled(rect, cr(12.0), p.shell_lo));
    let face = Rect::from_min_size(rect.min, vec2(rect.width(), rect.height() - depth));
    painter.set(face_idx, egui::epaint::RectShape::filled(face, cr(12.0), p.shell));
    let y = snap_px(painter, face.top() + 1.0);
    let hl = Rect::from_min_max(pos2(face.left() + 7.2, y), pos2(face.right() - 7.2, y + 1.0));
    painter.set(hl_idx, egui::epaint::RectShape::filled(hl, 0, p.shell_hi));
    if let Some(t) = title {
        paint_plate(ui.painter(), rect.min + vec2(14.0, 14.0), t, p.plate, p.plate_text);
    }
    ui.allocate_rect(rect, Sense::hover());
    (r, natural)
}

pub fn panel<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    panel_ex(ui, [14.0, 14.0, 14.0, 17.0], None, 0.0, add)
}

pub fn group<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    panel_ex(ui, [14.0, 14.0, 14.0, 17.0], Some(title), 0.0, add)
}

/// Background of a panel occupying `rect` exactly (when the size is known up front).
pub fn paint_panel(painter: &Painter, rect: Rect) {
    let p = pal();
    paint_plastic(painter, rect, p.shell, p.shell_lo, Some(p.shell_hi), 3.0, 12.0, false);
}

// ---------------------------------------------------------------------- keys

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Normal,
    Accent,
    Flat,
}

/// Button / ToolButton: a key of the MiniToo keyboard.
pub struct Key<'a> {
    text: &'a str,
    icon: Option<&'a str>,
    kind: KeyKind,
    checked: bool,
    enabled: bool,
    height: Option<f32>,
    pad: Option<f32>,
    width: Option<f32>,
    tip: Option<&'a str>,
    bold: Option<bool>,
}

impl<'a> Key<'a> {
    pub fn new(text: &'a str) -> Self {
        Key { text, icon: None, kind: KeyKind::Normal, checked: false, enabled: true, height: None, pad: None, width: None, tip: None, bold: None }
    }
    pub fn icon_only(icon: &'a str) -> Self {
        Key::new("").icon(icon)
    }
    pub fn icon(mut self, name: &'a str) -> Self {
        self.icon = Some(name);
        self
    }
    pub fn accent(mut self, on: bool) -> Self {
        if on {
            self.kind = KeyKind::Accent;
        }
        self
    }
    pub fn flat(mut self) -> Self {
        self.kind = KeyKind::Flat;
        self
    }
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = Some(h);
        self
    }
    pub fn pad(mut self, p: f32) -> Self {
        self.pad = Some(p);
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
    pub fn tip(mut self, t: &'a str) -> Self {
        self.tip = Some(t);
        self
    }
    pub fn bold(mut self) -> Self {
        self.bold = Some(true);
        self
    }

    fn font(&self) -> FontId {
        if self.bold.unwrap_or(false) { bold(13.0) } else { font(13.0) }
    }

    fn spacing(&self) -> f32 {
        if self.kind == KeyKind::Flat { 6.0 } else { 7.0 }
    }

    pub fn size(&self, ui: &Ui) -> Vec2 {
        let flat = self.kind == KeyKind::Flat;
        let icon_only = self.text.is_empty() && self.icon.is_some();
        let pad = self.pad.unwrap_or(if flat { if icon_only { 6.0 } else { 10.0 } } else { 12.0 });
        let mut content = 0.0;
        if self.icon.is_some() {
            content += 12.0;
        }
        if !self.text.is_empty() {
            if self.icon.is_some() {
                content += self.spacing();
            }
            content += text_width(ui, self.text, self.font());
        }
        let h = self.height.unwrap_or(if flat { 30.0 } else { 33.0 });
        let w = self.width.unwrap_or((content + 2.0 * pad).max(if flat { 30.0 } else { 40.0 }));
        vec2(w, h)
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let size = self.size(ui);
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        self.show_at(ui, rect)
    }

    pub fn show_at(self, ui: &mut Ui, rect: Rect) -> Response {
        let p = pal();
        let sense = if self.enabled { Sense::click() } else { Sense::hover() };
        let id = ui.next_auto_id();
        ui.skip_ahead_auto_ids(1);
        let mut resp = ui.interact(rect, id, sense);
        if self.enabled {
            resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        let hovered = self.enabled && resp.hovered();
        let down = self.enabled && resp.is_pointer_button_down_on();
        let sunk = down || self.checked;
        let painter = ui.painter();
        let (depth, sink, color) = match self.kind {
            KeyKind::Flat => {
                let bg = if sunk {
                    if p.dark { Color32::from_white_alpha(40) } else { Color32::from_black_alpha(36) }
                } else if hovered {
                    p.hover
                } else {
                    Color32::TRANSPARENT
                };
                painter.rect_filled(rect, 6, bg);
                let c = if !self.enabled {
                    p.text_disabled
                } else if self.checked {
                    p.accent_text
                } else {
                    p.text
                };
                (0.0, 0.0, c)
            }
            KeyKind::Normal | KeyKind::Accent => {
                let accent = self.kind == KeyKind::Accent;
                let face = if accent {
                    if hovered { p.accent_hi } else { p.accent }
                } else if hovered {
                    p.key_hi
                } else {
                    p.key
                };
                let edge = if accent { p.accent_edge } else { p.key_edge };
                let op = if self.enabled { 1.0 } else { 0.6 };
                paint_plastic(painter, rect, fade(face, op), fade(edge, op), Some(fade(lighter(face, 1.07), op)), 3.0, 6.0, sunk);
                let c = if !self.enabled {
                    p.text_disabled
                } else if accent {
                    WHITE
                } else if self.checked {
                    p.accent_text
                } else {
                    p.key_text
                };
                (3.0, if sunk { 2.0 } else { 0.0 }, c)
            }
        };
        let cy = rect.top() + (rect.height() - depth) / 2.0 + sink;
        let g = (!self.text.is_empty()).then(|| galley(ui, self.text, self.font(), color));
        let mut w = 0.0;
        if self.icon.is_some() {
            w += 12.0;
        }
        if let Some(g) = &g {
            if self.icon.is_some() {
                w += self.spacing();
            }
            w += g.size().x;
        }
        let mut x = rect.center().x - w / 2.0;
        let painter = ui.painter();
        if let Some(icon) = self.icon {
            pixel::paint_icon(painter, pos2(x, cy - 6.0), icon, 1, color, None);
            x += 12.0 + self.spacing();
        }
        if let Some(g) = g {
            let gy = (cy - g.size().y / 2.0).round();
            painter.galley(pos2(x.round(), gy), g, color);
        }
        match self.tip {
            Some(t) => tip_disabled(resp, t),
            None => resp,
        }
    }
}

/// TabBar of keys with equal widths; returns the clicked index.
pub fn tabs(ui: &mut Ui, items: &[(&str, Option<&str>)], current: usize) -> Option<usize> {
    let make = |i: usize| {
        let (t, icon) = items[i];
        let mut k = Key::new(t);
        if let Some(ic) = icon {
            k = k.icon(ic);
        }
        k
    };
    let widest = (0..items.len()).map(|i| make(i).size(ui).x).fold(0.0, f32::max);
    let n = items.len() as f32;
    let (rect, _) = ui.allocate_exact_size(vec2(widest * n + 4.0 * (n - 1.0).max(0.0), 33.0), Sense::hover());
    let mut clicked = None;
    for i in 0..items.len() {
        let on = i == current;
        let r = Rect::from_min_size(pos2(rect.left() + i as f32 * (widest + 4.0), rect.top()), vec2(widest, 33.0));
        if make(i).width(widest).accent(on).checked(on).show_at(ui, r).clicked() && !on {
            clicked = Some(i);
        }
    }
    clicked
}

/// A column of fixed size inside a row (keeps the row's vertical centring).
pub fn col<R>(ui: &mut Ui, width: f32, height: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
    child.spacing_mut().item_spacing.y = 0.0;
    add(&mut child)
}

/// Small square key on thumbnails (MiniKey 22×22) or anywhere a compact icon key is needed.
pub fn mini_key(ui: &mut Ui, rect: Rect, icon: &str, lit: bool, tip_text: &str) -> Response {
    let p = pal();
    let id = ui.next_auto_id();
    ui.skip_ahead_auto_ids(1);
    let resp = ui.interact(rect, id, Sense::click());
    let hovered = resp.hovered();
    let down = resp.is_pointer_button_down_on();
    let face = if lit {
        if hovered { p.accent_hi } else { p.accent }
    } else if hovered {
        p.key_hi
    } else {
        p.key
    };
    let edge = if lit { p.accent_edge } else { p.key_edge };
    let painter = ui.painter();
    paint_plastic(painter, rect, face, edge, None, 2.0, 5.0, down);
    let cy = rect.top() + (rect.height() - 2.0) / 2.0 + if down { 1.0 } else { -1.0 };
    pixel::paint_icon(painter, pos2(rect.center().x - 6.0, cy - 6.0), icon, 1, if lit { WHITE } else { p.key_text }, None);
    tip(resp, tip_text)
}

// ---------------------------------------------------------------------- switches and checks

/// Switch with a label; returns the response (clicked = toggle).
pub fn switch(ui: &mut Ui, on: bool, label: &str, enabled: bool) -> Response {
    let p = pal();
    let font = font(13.0);
    let avail = ui.available_width();
    let text_w = (avail - 4.0 - 42.0 - 10.0 - 4.0).max(40.0);
    let g = (!label.is_empty()).then(|| galley_wrapped(ui, label, font.clone(), if enabled { p.text } else { p.text_disabled }, text_w));
    let tw = g.as_ref().map(|g| g.size().x + 10.0).unwrap_or(0.0);
    let th = g.as_ref().map(|g| g.size().y).unwrap_or(0.0);
    let size = vec2(4.0 + 42.0 + tw + 4.0, (22.0f32).max(th) + 8.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let t = ui.ctx().animate_bool_with_time(resp.id, on, 0.11);
    let painter = ui.painter();
    let op = if enabled { 1.0 } else { 0.5 };
    let track = Rect::from_min_size(pos2(rect.left() + 4.0, rect.center().y - 11.0), vec2(42.0, 22.0));
    paint_well(painter, track, 6.0, false, fade(if on { p.accent } else { p.well }, op), fade(if on { p.accent_edge } else { p.well_line }, op));
    let x = track.left() + 1.0 + t * (42.0 - 22.0);
    let knob = Rect::from_min_size(pos2(x, track.top() + 1.0), vec2(20.0, 20.0));
    let hovered = enabled && resp.hovered();
    let down = enabled && resp.is_pointer_button_down_on();
    let face = paint_plastic(painter, knob, fade(if hovered { p.key_hi } else { p.key }, op), fade(p.key_edge, op), None, 2.0, 5.0, down);
    for i in 0..3 {
        let y = snap_px(painter, face.center().y - 3.5 + i as f32 * 3.0);
        painter.rect_filled(Rect::from_min_size(pos2(face.center().x - 4.0, y), vec2(8.0, 1.0)), 0, fade(p.key_edge, 0.7 * op));
    }
    if let Some(g) = g {
        let pos = pos2(track.right() + 10.0, rect.center().y - g.size().y / 2.0);
        painter.galley(pos, g, p.text);
    }
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// CheckBox; empty label = just the box (28 px wide).
pub fn checkbox(ui: &mut Ui, on: bool, label: &str, enabled: bool) -> Response {
    let p = pal();
    let avail = ui.available_width();
    let g = (!label.is_empty())
        .then(|| galley_wrapped(ui, label, font(13.0), if enabled { p.text } else { p.text_disabled }, (avail - 37.0).max(40.0)));
    let tw = g.as_ref().map(|g| g.size().x + 9.0).unwrap_or(0.0);
    let th = g.as_ref().map(|g| g.size().y).unwrap_or(0.0);
    let size = vec2(4.0 + 20.0 + tw + 4.0, 20.0f32.max(th) + 8.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    paint_checkbox(ui, rect, on, enabled, resp.hovered());
    if let Some(g) = g {
        ui.painter().galley(pos2(rect.left() + 33.0, rect.center().y - g.size().y / 2.0), g, p.text);
    }
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

fn paint_checkbox(ui: &Ui, rect: Rect, on: bool, enabled: bool, hovered: bool) {
    let p = pal();
    let op = if enabled { 1.0 } else { 0.5 };
    let b = Rect::from_min_size(pos2(rect.left() + 4.0, rect.center().y - 10.0), vec2(20.0, 20.0));
    let line = if on {
        p.accent_edge
    } else if hovered && enabled {
        p.text_dim
    } else {
        p.well_line
    };
    paint_well(ui.painter(), b, 4.0, false, fade(if on { p.accent } else { p.well }, op), fade(line, op));
    if on {
        pixel::paint_icon(ui.painter(), b.center() - vec2(6.0, 6.0), "check", 1, fade(WHITE, op), None);
    }
}

// ---------------------------------------------------------------------- slider, progress, busy

/// Slider: recessed track, orange fill, a small key as the handle. Returns (response, value).
#[allow(clippy::too_many_arguments)]
pub fn slider(ui: &mut Ui, value: f32, min: f32, max: f32, step: f32, width: f32, enabled: bool, tip_text: &str) -> (Response, f32) {
    let p = pal();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 30.0), if enabled { Sense::click_and_drag() } else { Sense::hover() });
    let pad = 4.0;
    let avail = rect.width() - 2.0 * pad;
    let mut v = value.clamp(min, max);
    if enabled && (resp.dragged() || resp.is_pointer_button_down_on()) {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - rect.left() - pad - 7.0) / (avail - 14.0)).clamp(0.0, 1.0);
            let raw = min + t * (max - min);
            v = ((raw - min) / step).round() * step + min;
            v = v.clamp(min, max);
        }
    }
    let pos = if max > min { (v - min) / (max - min) } else { 0.0 };
    let op = if enabled { 1.0 } else { 0.5 };
    let painter = ui.painter();
    let track = Rect::from_min_size(pos2(rect.left() + pad, rect.center().y - 4.0), vec2(avail, 8.0));
    paint_well(painter, track, 4.0, false, fade(p.well, op), fade(p.well_line, op));
    let fw = (pos * track.width() - 2.0).max(0.0);
    if fw > 0.0 {
        painter.rect_filled(Rect::from_min_size(track.min + vec2(1.0, 1.0), vec2(fw, 6.0)), 3, fade(p.accent, op));
    }
    let hx = rect.left() + pad + pos * (avail - 14.0);
    let handle = Rect::from_min_size(pos2(hx, rect.center().y - 11.0), vec2(14.0, 22.0));
    let hovered = enabled && (resp.hovered() || resp.dragged());
    let face = paint_plastic(painter, handle, fade(if hovered { p.key_hi } else { p.key }, op), fade(p.key_edge, op), None, 3.0, 3.0, resp.dragged());
    for i in 0..3 {
        let y = snap_px(painter, face.center().y - 3.5 + i as f32 * 3.0);
        painter.rect_filled(Rect::from_min_size(pos2(face.center().x - 3.0, y), vec2(6.0, 1.0)), 0, fade(p.key_edge, op));
    }
    let resp = if tip_text.is_empty() { resp } else { tip(resp, tip_text) };
    (resp, v)
}

/// Segmented progress bar in a well.
pub fn progress(ui: &mut Ui, frac: f32, width: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 14.0), Sense::hover());
    let painter = ui.painter();
    paint_well(painter, rect, 3.0, false, pal().well, pal().well_line);
    let inner = rect.shrink(3.0);
    let n = ((inner.width() + 2.0) / 7.0).floor().max(1.0) as usize;
    for i in 0..n {
        if (i as f32 + 0.5) / n as f32 <= frac {
            let r = Rect::from_min_size(pos2(inner.left() + i as f32 * 7.0, inner.top()), vec2(5.0, inner.height()));
            painter.rect_filled(r, 0, color);
        }
    }
    resp
}

/// Three blinking pixels.
pub fn busy(ui: &mut Ui) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 14.0), Sense::hover());
    let t = ui.input(|i| i.time);
    let step = ((t / 0.16) as usize) % 3;
    ui.ctx().request_repaint_after(Duration::from_millis(80));
    let a = pal().accent;
    for i in 0..3 {
        let r = Rect::from_min_size(pos2(rect.left() + 2.0 + i as f32 * 9.0, rect.center().y - 3.0), vec2(6.0, 6.0));
        ui.painter().rect_filled(r, 0, if i == step { a } else { fade(a, 0.3) });
    }
    resp
}

// ---------------------------------------------------------------------- text fields

pub struct Field<'a> {
    pub hint: &'a str,
    pub width: f32,
    pub mono: bool,
    pub password: bool,
    pub enabled: bool,
    pub id: Option<Id>,
}

impl Default for Field<'_> {
    fn default() -> Self {
        Field { hint: "", width: 160.0, mono: false, password: false, enabled: true, id: None }
    }
}

/// TextField in a well.
pub fn text_field(ui: &mut Ui, buf: &mut String, f: Field) -> Response {
    let p = pal();
    let (rect, _) = ui.allocate_exact_size(vec2(f.width, 32.0), Sense::hover());
    let well_idx = ui.painter().add(Shape::Noop);
    let fnt = if f.mono { mono(13.0) } else { font(13.0) };
    let mut te = egui::TextEdit::singleline(buf)
        .frame(egui::Frame::NONE)
        .margin(Margin::ZERO)
        .font(fnt.clone())
        .text_color(if f.enabled { p.text } else { p.text_disabled })
        .hint_text(egui::RichText::new(f.hint).color(p.text_dim).font(fnt))
        .password(f.password)
        .desired_width(rect.width() - 20.0)
        .vertical_align(Align::Center)
        .interactive(f.enabled);
    if let Some(id) = f.id {
        te = te.id(id);
    }
    let inner = Rect::from_min_max(rect.min + vec2(10.0, 6.0), rect.max - vec2(10.0, 6.0));
    let resp = ui.new_child(UiBuilder::new().max_rect(inner)).add_sized(inner.size(), te);
    let focused = resp.has_focus();
    let mut shapes = Vec::new();
    let op = if f.enabled { 1.0 } else { 0.6 };
    let fill = fade(p.well, op);
    shapes.push(Shape::Rect(egui::epaint::RectShape::filled(rect, cr(6.0), fill)));
    let bw = if focused { 2.0 } else { 1.0 };
    let sh = Rect::from_min_size(pos2(rect.left() + 3.0, rect.top() + bw), vec2(rect.width() - 6.0, 2.0));
    shapes.push(Shape::Rect(egui::epaint::RectShape::filled(sh, 0, Color32::from_black_alpha(if p.dark { 64 } else { 18 }))));
    let stroke = if focused { Stroke::new(2.0, p.accent) } else { Stroke::new(1.0, fade(p.well_line, op)) };
    shapes.push(Shape::Rect(egui::epaint::RectShape::stroke(rect, cr(6.0), stroke, StrokeKind::Inside)));
    ui.painter().set(well_idx, Shape::Vec(shapes));
    resp
}

/// Enter was pressed in this (single-line) field.
pub fn submitted(ui: &Ui, resp: &Response) -> bool {
    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// SpinBox: value between "−" and "+" keys. Returns the new value when changed.
pub struct Spin<'a> {
    pub value: i64,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub width: f32,
    pub editable: bool,
    pub enabled: bool,
    pub fmt: Option<&'a dyn Fn(i64) -> String>,
    pub id_salt: &'a str,
}

impl<'a> Spin<'a> {
    pub fn new(id_salt: &'a str, value: i64, min: i64, max: i64) -> Self {
        Spin { value, min, max, step: 1, width: 120.0, editable: true, enabled: true, fmt: None, id_salt }
    }
    pub fn step(mut self, s: i64) -> Self {
        self.step = s;
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
    pub fn fmt(mut self, f: &'a dyn Fn(i64) -> String) -> Self {
        self.fmt = Some(f);
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Option<i64> {
        let p = pal();
        let (rect, _) = ui.allocate_exact_size(vec2(self.width, 33.0), Sense::hover());
        let id = ui.make_persistent_id(self.id_salt);
        let op = if self.enabled { 1.0 } else { 0.6 };
        let text_of = |v: i64| self.fmt.map(|f| f(v)).unwrap_or_else(|| v.to_string());
        let edit_id = id.with("edit");
        let focused = ui.memory(|m| m.has_focus(edit_id));
        let well_rect = rect;
        paint_well(ui.painter(), well_rect, 6.0, focused, fade(p.well, op), fade(p.well_line, op));
        let mut out = None;
        let keys = [(Rect::from_min_size(rect.min, vec2(28.0, rect.height())), "minus", -1i64), (
            Rect::from_min_size(pos2(rect.right() - 28.0, rect.top()), vec2(28.0, rect.height())),
            "plus",
            1,
        )];
        for (r, icon, dir) in keys {
            let can = self.enabled && if dir < 0 { self.value > self.min } else { self.value < self.max };
            let resp = ui.interact(r, id.with(icon), if self.enabled { Sense::click() } else { Sense::hover() });
            let hovered = self.enabled && resp.hovered();
            let down = self.enabled && resp.is_pointer_button_down_on();
            let kop = if can || dir < 0 { op } else { 0.5 };
            let face = paint_plastic(ui.painter(), r, fade(if hovered { p.key_hi } else { p.key }, kop), fade(p.key_edge, kop), None, 3.0, 6.0, down);
            pixel::paint_icon(ui.painter(), face.center() - vec2(6.0, 6.0), icon, 1, fade(p.key_text, kop), None);
            if resp.clicked() && can {
                out = Some((self.value + dir * self.step).clamp(self.min, self.max));
            }
        }
        let mid = Rect::from_min_max(pos2(rect.left() + 34.0, rect.top() + 4.0), pos2(rect.right() - 34.0, rect.bottom() - 4.0));
        let fnt = font(13.0);
        if self.editable && self.enabled {
            let mut buf: String = if focused {
                ui.data_mut(|d| d.get_temp::<String>(edit_id)).unwrap_or_else(|| text_of(self.value))
            } else {
                text_of(self.value)
            };
            let te = egui::TextEdit::singleline(&mut buf)
                .id(edit_id)
                .frame(egui::Frame::NONE)
                .margin(Margin::ZERO)
                .font(fnt)
                .text_color(p.text)
                .horizontal_align(Align::Center)
                .vertical_align(Align::Center)
                .desired_width(mid.width());
            let resp = ui.new_child(UiBuilder::new().max_rect(mid)).add_sized(mid.size(), te);
            if resp.gained_focus() {
                buf = self.value.to_string();
            }
            if resp.has_focus() {
                ui.data_mut(|d| d.insert_temp(edit_id, buf.clone()));
            }
            if resp.lost_focus() {
                let digits: String = buf.chars().filter(|c| c.is_ascii_digit() || *c == '-').collect();
                if let Ok(v) = digits.parse::<i64>() {
                    let v = v.clamp(self.min, self.max);
                    if v != self.value {
                        out = Some(v);
                    }
                }
                ui.data_mut(|d| d.remove::<String>(edit_id));
            }
        } else {
            let g = galley(ui, &text_of(self.value), fnt, if self.enabled { p.text } else { p.text_disabled });
            ui.painter().galley(mid.center() - g.size() / 2.0, g, p.text);
        }
        out
    }
}

/// ComboBox: a key with the current item and a popup list. Returns the picked index.
pub fn combo(ui: &mut Ui, id_salt: &str, items: &[&str], current: usize, width: f32) -> Option<usize> {
    let p = pal();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 33.0), Sense::hover());
    let id = ui.make_persistent_id(id_salt);
    let resp = ui.interact(rect, id, Sense::click());
    let popup_id = egui::Popup::default_response_id(&resp);
    let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let hovered = resp.hovered();
    let sunk = resp.is_pointer_button_down_on() || open;
    let face = paint_plastic(ui.painter(), rect, if hovered { p.key_hi } else { p.key }, p.key_edge, None, 3.0, 6.0, sunk);
    let label = items.get(current).copied().unwrap_or("");
    let g = galley_elided(ui, label, font(13.0), p.key_text, width - 12.0 - 12.0 - 22.0);
    ui.painter().galley(pos2(face.left() + 12.0, face.center().y - g.size().y / 2.0), g, p.key_text);
    pixel::paint_icon(ui.painter(), pos2(face.right() - 22.0, face.center().y - 6.0), if open { "up" } else { "down" }, 1, p.key_text, None);
    let mut picked = None;
    let frame = egui::Frame::NONE
        .fill(p.shell)
        .stroke(Stroke::new(1.0, p.shell_lo))
        .corner_radius(8)
        .inner_margin(Margin::same(4))
        .shadow(egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(40) });
    egui::Popup::from_toggle_button_response(&resp).frame(frame).width(width).gap(2.0).show(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        for (i, it) in items.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(width - 8.0, 32.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, 6, p.hover);
            }
            if i == current {
                ui.painter().rect_filled(r, 6, p.selection);
            }
            let fnt = if i == current { bold(13.0) } else { font(13.0) };
            let g = galley_elided(ui, it, fnt, p.text, r.width() - 20.0);
            ui.painter().galley(pos2(r.left() + 10.0, r.center().y - g.size().y / 2.0), g, p.text);
            if resp.clicked() {
                picked = Some(i);
                ui.close();
            }
        }
    });
    picked
}

/// ItemDelegate-like clickable row; returns the response.
pub fn item_row(ui: &mut Ui, label: &str, highlighted: bool, width: f32) -> Response {
    let p = pal();
    let (r, resp) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    let bg = if resp.is_pointer_button_down_on() || highlighted || resp.has_focus() {
        p.selection
    } else if resp.hovered() {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(r, 6, bg);
    let c = if highlighted || resp.has_focus() { p.accent_text } else { p.text };
    let g = galley_elided(ui, label, font(13.0), c, r.width() - 20.0);
    ui.painter().galley(pos2(r.left() + 10.0, r.center().y - g.size().y / 2.0), g, c);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Horizontal separator line.
pub fn separator(ui: &mut Ui, margin: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0 + 2.0 * margin), Sense::hover());
    let y = r.center().y;
    ui.painter().rect_filled(Rect::from_min_max(pos2(r.left() + margin, y), pos2(r.right() - margin, y + 1.0)), 0, pal().shell_lo);
}

/// PageHeader: pixel title ×2, subtitle; `actions` are laid out on the right.
pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str, actions: impl FnOnce(&mut Ui)) {
    let p = pal();
    let st = PixStyle::new(12).zoom(2);
    let title_h = st.height(title);
    let h = title_h + 3.0 + 16.0;
    let left = ui.available_rect_before_wrap().min;
    let full_w = ui.available_width();
    let r = ui.allocate_ui_with_layout(vec2(full_w, h), Layout::right_to_left(Align::Center), |ui| {
        ui.set_min_height(h);
        ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
        let before = ui.cursor().right();
        actions(ui);
        before - ui.cursor().right()
    });
    let actions_w = r.inner.max(0.0);
    let top = left.y;
    pixel::paint_text(ui.painter(), pos2(left.x, top), title, &st, p.text);
    if !subtitle.is_empty() {
        let g = galley_elided(ui, subtitle, font(13.0), p.text_dim, full_w - actions_w - 14.0);
        ui.painter().galley(pos2(left.x, top + title_h + 3.0), g, p.text_dim);
    }
}

/// Mouse-wheel movement this frame in points (lines count as 40 px).
pub fn wheel_delta(ui: &Ui) -> Vec2 {
    ui.input(|i| {
        i.raw.events.iter().fold(Vec2::ZERO, |acc, e| match e {
            egui::Event::MouseWheel { unit, delta, .. } => {
                acc + match unit {
                    egui::MouseWheelUnit::Point => *delta,
                    egui::MouseWheelUnit::Line => *delta * 40.0,
                    egui::MouseWheelUnit::Page => *delta * 400.0,
                }
            }
            _ => acc,
        })
    })
}

pub fn tabs_width(ui: &Ui, items: &[(&str, Option<&str>)]) -> f32 {
    let widest = items
        .iter()
        .map(|(t, i)| {
            let mut k = Key::new(t);
            if let Some(i) = i {
                k = k.icon(i);
            }
            k.size(ui).x
        })
        .fold(0.0, f32::max);
    widest * items.len() as f32 + 4.0 * (items.len() as f32 - 1.0).max(0.0)
}

/// Keys (with an optional leading label) wrapped onto as many rows as needed. Returns the
/// index of the clicked key.
pub fn flow_keys(ui: &mut Ui, label: Option<&str>, keys: Vec<Key>, gap: f32) -> Option<usize> {
    let p = pal();
    let width = ui.available_width();
    let mut items: Vec<Vec2> = Vec::new();
    let label_w = label.map(|l| text_width(ui, l, font(13.0)));
    if let Some(lw) = label_w {
        items.push(vec2(lw, 33.0));
    }
    let key_sizes: Vec<Vec2> = keys.iter().map(|k| k.size(ui)).collect();
    items.extend(key_sizes.iter().copied());
    let mut pos = Vec::with_capacity(items.len());
    let (mut x, mut y, mut row_h) = (0.0f32, 0.0f32, 0.0f32);
    for s in &items {
        if x > 0.0 && x + s.x > width {
            x = 0.0;
            y += row_h + gap;
            row_h = 0.0;
        }
        pos.push(vec2(x, y));
        x += s.x + gap;
        row_h = row_h.max(s.y);
    }
    let (rect, _) = ui.allocate_exact_size(vec2(width, y + row_h), Sense::hover());
    let mut clicked = None;
    let mut it = pos.into_iter();
    if let Some(l) = label {
        let o = it.next().unwrap_or_default();
        let g = galley(ui, l, font(13.0), p.text_dim);
        ui.painter().galley(rect.min + o + vec2(0.0, 16.5 - g.size().y / 2.0), g, p.text_dim);
    }
    for (i, (k, (o, s))) in keys.into_iter().zip(it.zip(key_sizes)).enumerate() {
        let r = Rect::from_min_size(rect.min + o + vec2(0.0, (33.0 - s.y) / 2.0), s);
        if k.show_at(ui, r).clicked() {
            clicked = Some(i);
        }
    }
    clicked
}
