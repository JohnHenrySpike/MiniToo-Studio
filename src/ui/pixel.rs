//! PixelText and PixelIcon (§16.3): small bitmaps (white masks, tinted when painted) scaled by
//! whole numbers without smoothing. Textures are cached per text/size and per icon.
//!
//! A magnified pixel always covers a whole number of physical pixels. With a fractional display
//! scale (150%) DejaVu text is rasterised again at the physical size instead of being stretched;
//! the 5×7 font and the icons round their pixel to whole physical pixels.

use crate::canvas::Canvas;
use crate::color::Color;
use crate::fonts::FontSpec;
use crate::pixelfont;
use egui::{Color32, Painter, Pos2, Rect, TextureHandle, TextureId, TextureOptions, Vec2, pos2, vec2};
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone, PartialEq, Eq, Hash)]
struct TextKey {
    text: String,
    /// rasterised pixel size (`f32` bits): the style size, or larger for a fractional scale
    px: u32,
    bold: bool,
    spacing: i32,
}

#[derive(Default)]
struct Cache {
    texts: HashMap<TextKey, (TextureHandle, [usize; 2])>,
    icons: HashMap<String, Option<(TextureHandle, TextureHandle)>>,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
    static PPP: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

/// Physical pixels per point of the window being drawn; set once per frame.
pub fn set_pixels_per_point(ppp: f32) {
    PPP.with(|p| p.set(ppp.max(0.1)));
}

fn ppp() -> f32 {
    PPP.with(|p| p.get())
}

/// How one font pixel magnified by `zoom` lands on the screen.
#[derive(Clone, Copy)]
enum Scale {
    /// whole physical pixels per font pixel
    Exact,
    /// fractional: rasterise at `px_factor` × the size, then magnify by `block` physical pixels
    Rerender { px_factor: f32, block: f32 },
}

fn scale_of(zoom: u32, ppp: f32) -> Scale {
    let s = zoom as f32 * ppp;
    if (s - s.round()).abs() < 0.01 {
        Scale::Exact
    } else {
        let block = s.floor().max(1.0);
        Scale::Rerender { px_factor: s / block, block }
    }
}

/// Points per font pixel of a bitmap (5×7 font, icon): rounded to whole physical pixels.
fn bitmap_unit(zoom: u32, ppp: f32) -> f32 {
    (zoom as f32 * ppp).round().max(1.0) / ppp
}

/// Forget all textures (a new egui context is about to be used).
pub fn reset() {
    CACHE.with(|c| *c.borrow_mut() = Cache::default());
}

/// Text style of a PixelText.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixStyle {
    pub size: u32,
    pub zoom: u32,
    pub bold: bool,
    pub spacing: i32,
}

impl PixStyle {
    pub const fn new(size: u32) -> Self {
        PixStyle { size, zoom: 1, bold: true, spacing: 0 }
    }
    pub const fn zoom(mut self, z: u32) -> Self {
        self.zoom = z;
        self
    }
    pub const fn regular(mut self) -> Self {
        self.bold = false;
        self
    }
    pub const fn spacing(mut self, s: i32) -> Self {
        self.spacing = s;
        self
    }

    fn bitmap(&self, text: &str) -> bool {
        self.size <= 9 && pixelfont::covers(text)
    }

    fn font(&self) -> FontSpec {
        self.font_px(self.size as f32)
    }

    fn font_px(&self, px: f32) -> FontSpec {
        let f = if self.bold { FontSpec::bold(px) } else { FontSpec::sans(px) };
        f.no_aa()
    }

    /// Points per font pixel of this text on the current screen.
    fn unit(&self, text: &str) -> f32 {
        if self.bitmap(text) || (self.size <= 9 && text.is_empty()) { bitmap_unit(self.zoom, ppp()) } else { self.zoom as f32 }
    }

    /// Size in font pixels (zoom 1).
    pub fn measure_raw(&self, text: &str) -> Vec2 {
        if text.is_empty() {
            return Vec2::ZERO;
        }
        if self.bitmap(text) {
            return vec2(pixelfont::width(text, self.spacing) as f32, pixelfont::HEIGHT as f32);
        }
        let f = self.font();
        let w = Canvas::text_width(text, f) + self.spacing as f32 * text.chars().count().saturating_sub(1) as f32;
        vec2(w.ceil(), f.height().ceil())
    }

    /// Size in points.
    pub fn measure(&self, text: &str) -> Vec2 {
        self.measure_raw(text) * self.unit(text)
    }

    /// Line height in points (also for empty text).
    pub fn height(&self, text: &str) -> f32 {
        if self.bitmap(text) || (self.size <= 9 && text.is_empty()) {
            pixelfont::HEIGHT as f32 * self.unit(text)
        } else {
            self.font().height().ceil() * self.zoom as f32
        }
    }

    /// Elides with "…" so that the text fits `max_w` points.
    pub fn elide(&self, text: &str, max_w: f32) -> String {
        let avail = (max_w / self.unit(text)).floor();
        if self.measure_raw(text).x <= avail {
            return text.to_string();
        }
        if self.bitmap(text) {
            return pixelfont::elided(text, self.spacing, avail as i32);
        }
        let mut chars: Vec<char> = text.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let s = chars.iter().collect::<String>().trim_end().to_string() + "…";
            if self.measure_raw(&s).x <= avail {
                return s;
            }
        }
        "…".to_string()
    }
}

fn mask_texture(ctx: &egui::Context, name: &str, w: usize, h: usize, alpha: impl Fn(usize, usize) -> u8) -> TextureHandle {
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            px.push(Color32::from_white_alpha(alpha(x, y)));
        }
    }
    ctx.load_texture(name, egui::ColorImage::new([w, h], px), TextureOptions::NEAREST)
}

fn text_texture(ctx: &egui::Context, text: &str, st: &PixStyle, px: f32) -> (TextureId, [usize; 2]) {
    let key = TextKey { text: text.to_string(), px: px.to_bits(), bold: st.bold, spacing: st.spacing };
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((t, s)) = c.texts.get(&key) {
            return (t.id(), *s);
        }
        let canvas = if st.bitmap(text) {
            pixelfont::render(text, Color::WHITE, st.spacing)
        } else {
            let f = st.font_px(px);
            let spacing = (st.spacing as f32 * px / st.size as f32).round();
            let n = text.chars().count().saturating_sub(1) as f32;
            let w = Canvas::text_width(text, f) + spacing * n;
            let mut cv = Canvas::new((w.ceil() as u32).max(1), (f.height().ceil() as u32).max(1));
            if spacing == 0.0 {
                cv.text_tl(0.0, 0.0, text, f, Color::WHITE);
            } else {
                let mut x = 0.0;
                let mut buf = [0u8; 4];
                for ch in text.chars() {
                    let s = ch.encode_utf8(&mut buf);
                    x += cv.text_tl(x, 0.0, s, f, Color::WHITE) + spacing;
                }
            }
            cv
        };
        let rgba = canvas.to_rgba();
        let (cw, ch) = (canvas.width() as usize, canvas.height() as usize);
        let tex = mask_texture(ctx, &format!("ptext:{px}:{text}"), cw, ch, |x, y| if rgba[(y * cw + x) * 4 + 3] > 0 { 255 } else { 0 });
        let id = tex.id();
        c.texts.insert(key, (tex, [cw, ch]));
        (id, [cw, ch])
    })
}

fn icon_textures(ctx: &egui::Context, name: &str) -> Option<(TextureId, TextureId)> {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(e) = c.icons.get(name) {
            return e.as_ref().map(|(a, b)| (a.id(), b.id()));
        }
        let entry = crate::icons::template(name).map(|rows| {
            let at = |x: usize, y: usize, want: char| rows[y].chars().nth(x) == Some(want);
            let main = mask_texture(ctx, &format!("icon:{name}"), 12, 12, |x, y| if at(x, y, '#') { 255 } else { 0 });
            let sec = mask_texture(ctx, &format!("icon2:{name}"), 12, 12, |x, y| if at(x, y, '+') { 255 } else { 0 });
            (main, sec)
        });
        let r = entry.as_ref().map(|(a, b)| (a.id(), b.id()));
        c.icons.insert(name.to_string(), entry);
        r
    })
}

/// Rounds a position to whole physical pixels so magnified pixels stay sharp.
pub fn snap(painter: &Painter, p: Pos2) -> Pos2 {
    let ppp = painter.pixels_per_point();
    pos2((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp)
}

const UV: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

/// Paints pixel text with its top-left corner at `pos`. Returns the painted rect.
pub fn paint_text(painter: &Painter, pos: Pos2, text: &str, st: &PixStyle, color: Color32) -> Rect {
    if text.is_empty() {
        return Rect::from_min_size(pos, Vec2::ZERO);
    }
    let ppp = painter.pixels_per_point();
    let (px, unit) = match scale_of(st.zoom, ppp) {
        Scale::Rerender { px_factor, block } if !st.bitmap(text) => (st.size as f32 * px_factor, block / ppp),
        _ => (st.size as f32, bitmap_unit(st.zoom, ppp)),
    };
    let (tex, [w, h]) = text_texture(painter.ctx(), text, st, px);
    let rect = Rect::from_min_size(snap(painter, pos), vec2(w as f32, h as f32) * unit);
    painter.image(tex, rect, UV, color);
    rect
}

/// Pixel text aligned inside `rect` (vertically centred), elided to its width.
pub fn paint_text_in(painter: &Painter, rect: Rect, text: &str, st: &PixStyle, color: Color32, align: egui::Align) -> Rect {
    let s = st.elide(text, rect.width());
    let size = st.measure(&s);
    let x = match align {
        egui::Align::Min => rect.left(),
        egui::Align::Center => rect.center().x - size.x / 2.0,
        egui::Align::Max => rect.right() - size.x,
    };
    paint_text(painter, pos2(x, rect.center().y - size.y / 2.0), &s, st, color)
}

/// Paints a 12×12 icon scaled by `zoom` with its top-left at `pos`. `secondary` defaults to
/// the main colour at 45%. Returns false for unknown names.
pub fn paint_icon(painter: &Painter, pos: Pos2, name: &str, zoom: u32, color: Color32, secondary: Option<Color32>) -> bool {
    let Some((main, sec)) = icon_textures(painter.ctx(), name) else { return false };
    // whole physical pixels per icon pixel, centred on the place of the nominal size
    let side = 12.0 * bitmap_unit(zoom, painter.pixels_per_point());
    let nominal = 12.0 * zoom as f32;
    let rect = Rect::from_min_size(snap(painter, pos + Vec2::splat((nominal - side) / 2.0)), Vec2::splat(side));
    let sec_color = secondary.unwrap_or_else(|| color.gamma_multiply(0.45));
    painter.image(sec, rect, UV, sec_color);
    painter.image(main, rect, UV, color);
    true
}

/// Icon centred in `rect`.
pub fn paint_icon_centered(painter: &Painter, center: Pos2, name: &str, zoom: u32, color: Color32, secondary: Option<Color32>) {
    let s = 12.0 * zoom as f32;
    paint_icon(painter, center - Vec2::splat(s / 2.0), name, zoom, color, secondary);
}
