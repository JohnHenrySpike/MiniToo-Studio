//! PixelText and PixelIcon (§16.3): small bitmaps (white masks, tinted when painted) scaled by
//! whole numbers without smoothing. Textures are cached per text/size and per icon.

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
    size: u32,
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
        let f = if self.bold { FontSpec::bold(self.size as f32) } else { FontSpec::sans(self.size as f32) };
        f.no_aa()
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
        self.measure_raw(text) * self.zoom as f32
    }

    /// Line height in points (also for empty text).
    pub fn height(&self, text: &str) -> f32 {
        if self.bitmap(text) || (self.size <= 9 && text.is_empty()) {
            (pixelfont::HEIGHT as u32 * self.zoom) as f32
        } else {
            self.font().height().ceil() * self.zoom as f32
        }
    }

    /// Elides with "…" so that the text fits `max_w` points.
    pub fn elide(&self, text: &str, max_w: f32) -> String {
        let avail = (max_w / self.zoom as f32).floor();
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

fn text_texture(ctx: &egui::Context, text: &str, st: &PixStyle) -> (TextureId, [usize; 2]) {
    let key = TextKey { text: text.to_string(), size: st.size, bold: st.bold, spacing: st.spacing };
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((t, s)) = c.texts.get(&key) {
            return (t.id(), *s);
        }
        let raw = st.measure_raw(text);
        let (w, h) = ((raw.x as usize).max(1), (raw.y as usize).max(1));
        let canvas = if st.bitmap(text) {
            pixelfont::render(text, Color::WHITE, st.spacing)
        } else {
            let mut cv = Canvas::new(w as u32, h as u32);
            let mut f = st.font();
            f.aa = true;
            if st.spacing == 0 {
                cv.text_tl(0.0, 0.0, text, f, Color::WHITE);
            } else {
                let mut x = 0.0;
                let mut buf = [0u8; 4];
                for ch in text.chars() {
                    let s = ch.encode_utf8(&mut buf);
                    x += cv.text_tl(x, 0.0, s, f, Color::WHITE) + st.spacing as f32;
                }
            }
            cv
        };
        let rgba = canvas.to_rgba();
        let (cw, ch) = (canvas.width() as usize, canvas.height() as usize);
        // a slightly high cut-off thins the stems like hinted monochrome text
        let tex = mask_texture(ctx, &format!("ptext:{text}"), cw, ch, |x, y| if rgba[(y * cw + x) * 4 + 3] >= 158 { 255 } else { 0 });
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
    let (tex, [w, h]) = text_texture(painter.ctx(), text, st);
    let rect = Rect::from_min_size(snap(painter, pos), vec2(w as f32, h as f32) * st.zoom as f32);
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
    let rect = Rect::from_min_size(snap(painter, pos), Vec2::splat(12.0 * zoom as f32));
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
