//! Raster canvas for device screens (and anything else drawn offscreen): a thin, QPainter-like
//! layer over tiny-skia plus DejaVu text rendered with ab_glyph (hinted with skrifa when not
//! anti-aliased).
//!
//! Coordinates are in pixels, `f32`, origin top-left, like QPainter.

use crate::color::Color;
use crate::fonts::FontSpec;
use crate::frame::{Frame, HEIGHT, WIDTH};
use ab_glyph::{Font as _, ScaleFont as _};
use tiny_skia::{
    FillRule, FilterQuality, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder, Pattern, Pixmap,
    PixmapPaint, Point, Rect, SpreadMode, Stroke, Transform,
};

pub use tiny_skia::{LineCap, LineJoin};

/// Horizontal / vertical alignment inside a rectangle (Qt::AlignXxx).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VAlign {
    Top,
    Center,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Align(pub HAlign, pub VAlign);

impl Align {
    pub const LEFT: Align = Align(HAlign::Left, VAlign::Center);
    pub const CENTER: Align = Align(HAlign::Center, VAlign::Center);
    pub const RIGHT: Align = Align(HAlign::Right, VAlign::Center);
    pub const TOP_LEFT: Align = Align(HAlign::Left, VAlign::Top);
    pub const TOP_CENTER: Align = Align(HAlign::Center, VAlign::Top);
    pub const TOP_RIGHT: Align = Align(HAlign::Right, VAlign::Top);
    pub const BOTTOM_LEFT: Align = Align(HAlign::Left, VAlign::Bottom);
    pub const BOTTOM_CENTER: Align = Align(HAlign::Center, VAlign::Bottom);
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Shorthand for a rectangle.
pub const fn r(x: f32, y: f32, w: f32, h: f32) -> R {
    R { x, y, w, h }
}

impl R {
    fn skia(&self) -> Option<Rect> {
        Rect::from_xywh(self.x, self.y, self.w.max(0.0), self.h.max(0.0))
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn adjusted(&self, dx1: f32, dy1: f32, dx2: f32, dy2: f32) -> R {
        r(self.x + dx1, self.y + dy1, self.w - dx1 + dx2, self.h - dy1 + dy2)
    }
}

pub struct Canvas {
    pm: Pixmap,
    /// anti-aliasing for shapes (text has its own flag in [`FontSpec`])
    pub aa: bool,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Self {
        Canvas { pm: Pixmap::new(w.max(1), h.max(1)).expect("canvas size"), aa: true }
    }

    /// 160×128, filled black.
    pub fn device() -> Self {
        let mut c = Self::new(WIDTH as u32, HEIGHT as u32);
        c.fill(Color::BLACK);
        c
    }

    pub fn from_frame(f: &Frame) -> Self {
        let mut c = Self::new(WIDTH as u32, HEIGHT as u32);
        for (dst, src) in c.pm.data_mut().chunks_exact_mut(4).zip(f.rgb().chunks_exact(3)) {
            dst.copy_from_slice(&[src[0], src[1], src[2], 255]);
        }
        c
    }

    /// From straight RGBA.
    pub fn from_rgba(w: u32, h: u32, rgba: &[u8]) -> Self {
        let mut c = Self::new(w, h);
        for (dst, src) in c.pm.data_mut().chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
            let a = src[3] as u32;
            dst.copy_from_slice(&[
                (src[0] as u32 * a / 255) as u8,
                (src[1] as u32 * a / 255) as u8,
                (src[2] as u32 * a / 255) as u8,
                src[3],
            ]);
        }
        c
    }

    pub fn from_image(img: &image::RgbaImage) -> Self {
        Self::from_rgba(img.width(), img.height(), img.as_raw())
    }

    pub fn width(&self) -> u32 {
        self.pm.width()
    }
    pub fn height(&self) -> u32 {
        self.pm.height()
    }
    pub fn pixmap(&self) -> &Pixmap {
        &self.pm
    }
    pub fn pixmap_mut(&mut self) -> &mut Pixmap {
        &mut self.pm
    }

    /// Result as a device frame (composed over black). The canvas should be 160×128.
    pub fn to_frame(&self) -> Frame {
        if self.width() as usize == WIDTH && self.height() as usize == HEIGHT {
            Frame::from_rgba(self.pm.data(), true)
        } else {
            Frame::from_image(&self.to_image())
        }
    }

    /// Straight (demultiplied) RGBA.
    pub fn to_rgba(&self) -> Vec<u8> {
        self.pm.clone().take_demultiplied()
    }

    pub fn to_image(&self) -> image::RgbaImage {
        image::RgbaImage::from_raw(self.width(), self.height(), self.to_rgba()).expect("size")
    }

    fn paint(&self, c: Color) -> Paint<'static> {
        let mut p = Paint::default();
        p.set_color(c.to_skia());
        p.anti_alias = self.aa;
        p
    }

    // ------------------------------------------------------------------ shapes

    pub fn fill(&mut self, c: Color) {
        self.pm.fill(c.to_skia());
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Color) {
        if let Some(rect) = r(x, y, w, h).skia() {
            let p = self.paint(c);
            self.pm.fill_rect(rect, &p, Transform::identity(), None);
        }
    }

    pub fn fill_r(&mut self, rect: R, c: Color) {
        self.fill_rect(rect.x, rect.y, rect.w, rect.h, c);
    }

    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, c: Color) {
        if let Some(rect) = r(x, y, w, h).skia() {
            self.stroke_path(&PathBuilder::from_rect(rect), width, c);
        }
    }

    pub fn round_rect_path(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<Path> {
        let rad = radius.min(w / 2.0).min(h / 2.0).max(0.0);
        if rad <= 0.0 {
            return Some(PathBuilder::from_rect(r(x, y, w, h).skia()?));
        }
        // circular corners with cubic approximation
        let k = 0.552_284_8 * rad;
        let mut pb = PathBuilder::new();
        pb.move_to(x + rad, y);
        pb.line_to(x + w - rad, y);
        pb.cubic_to(x + w - rad + k, y, x + w, y + rad - k, x + w, y + rad);
        pb.line_to(x + w, y + h - rad);
        pb.cubic_to(x + w, y + h - rad + k, x + w - rad + k, y + h, x + w - rad, y + h);
        pb.line_to(x + rad, y + h);
        pb.cubic_to(x + rad - k, y + h, x, y + h - rad + k, x, y + h - rad);
        pb.line_to(x, y + rad);
        pb.cubic_to(x, y + rad - k, x + rad - k, y, x + rad, y);
        pb.close();
        pb.finish()
    }

    pub fn fill_round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, c: Color) {
        if let Some(path) = Self::round_rect_path(x, y, w, h, radius) {
            self.fill_path(&path, c);
        }
    }

    pub fn stroke_round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, width: f32, c: Color) {
        if let Some(path) = Self::round_rect_path(x, y, w, h, radius) {
            self.stroke_path(&path, width, c);
        }
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, c: Color) {
        if let Some(path) = PathBuilder::from_circle(cx, cy, radius) {
            self.fill_path(&path, c);
        }
    }

    pub fn stroke_circle(&mut self, cx: f32, cy: f32, radius: f32, width: f32, c: Color) {
        if let Some(path) = PathBuilder::from_circle(cx, cy, radius) {
            self.stroke_path(&path, width, c);
        }
    }

    /// Ellipse inscribed in the rectangle (QPainter::drawEllipse(QRectF)).
    pub fn fill_ellipse(&mut self, x: f32, y: f32, w: f32, h: f32, c: Color) {
        if let Some(path) = r(x, y, w, h).skia().and_then(PathBuilder::from_oval) {
            self.fill_path(&path, c);
        }
    }

    pub fn stroke_ellipse(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, c: Color) {
        if let Some(path) = r(x, y, w, h).skia().and_then(PathBuilder::from_oval) {
            self.stroke_path(&path, width, c);
        }
    }

    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, c: Color) {
        self.line_cap(x1, y1, x2, y2, width, c, LineCap::Butt);
    }

    pub fn line_cap(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, c: Color, cap: LineCap) {
        let mut pb = PathBuilder::new();
        pb.move_to(x1, y1);
        pb.line_to(x2, y2);
        if let Some(path) = pb.finish() {
            self.stroke_path_with(&path, width, c, cap, LineJoin::Miter);
        }
    }

    pub fn polyline(&mut self, pts: &[(f32, f32)], width: f32, c: Color) {
        if pts.len() < 2 {
            return;
        }
        let mut pb = PathBuilder::new();
        pb.move_to(pts[0].0, pts[0].1);
        for p in &pts[1..] {
            pb.line_to(p.0, p.1);
        }
        if let Some(path) = pb.finish() {
            self.stroke_path_with(&path, width, c, LineCap::Round, LineJoin::Round);
        }
    }

    pub fn fill_polygon(&mut self, pts: &[(f32, f32)], c: Color) {
        if pts.len() < 3 {
            return;
        }
        let mut pb = PathBuilder::new();
        pb.move_to(pts[0].0, pts[0].1);
        for p in &pts[1..] {
            pb.line_to(p.0, p.1);
        }
        pb.close();
        if let Some(path) = pb.finish() {
            self.fill_path(&path, c);
        }
    }

    /// Arc around (cx, cy). Angles in degrees, Qt convention: 0° = 3 o'clock, positive =
    /// counter-clockwise. `span` < 0 runs clockwise.
    pub fn arc(&mut self, cx: f32, cy: f32, radius: f32, start_deg: f32, span_deg: f32, width: f32, c: Color, cap: LineCap) {
        if span_deg.abs() < 0.01 {
            return;
        }
        let steps = ((span_deg.abs() / 4.0).ceil() as usize).max(2);
        let mut pb = PathBuilder::new();
        for i in 0..=steps {
            let a = (start_deg + span_deg * i as f32 / steps as f32).to_radians();
            let (x, y) = (cx + radius * a.cos(), cy - radius * a.sin());
            if i == 0 {
                pb.move_to(x, y);
            } else {
                pb.line_to(x, y);
            }
        }
        if let Some(path) = pb.finish() {
            self.stroke_path_with(&path, width, c, cap, LineJoin::Round);
        }
    }

    pub fn fill_path(&mut self, path: &Path, c: Color) {
        let p = self.paint(c);
        self.pm.fill_path(path, &p, FillRule::Winding, Transform::identity(), None);
    }

    pub fn fill_path_evenodd(&mut self, path: &Path, c: Color) {
        let p = self.paint(c);
        self.pm.fill_path(path, &p, FillRule::EvenOdd, Transform::identity(), None);
    }

    pub fn stroke_path(&mut self, path: &Path, width: f32, c: Color) {
        self.stroke_path_with(path, width, c, LineCap::Butt, LineJoin::Miter);
    }

    pub fn stroke_path_with(&mut self, path: &Path, width: f32, c: Color, cap: LineCap, join: LineJoin) {
        let p = self.paint(c);
        let stroke = Stroke { width, line_cap: cap, line_join: join, ..Stroke::default() };
        self.pm.stroke_path(path, &p, &stroke, Transform::identity(), None);
    }

    /// Vertical linear gradient over the rectangle.
    pub fn vgradient(&mut self, x: f32, y: f32, w: f32, h: f32, top: Color, bottom: Color) {
        self.gradient_rect(r(x, y, w, h), (x, y), (x, y + h), &[(0.0, top), (1.0, bottom)]);
    }

    /// Fills `rect` with a linear gradient from `p0` to `p1`.
    pub fn gradient_rect(&mut self, rect: R, p0: (f32, f32), p1: (f32, f32), stops: &[(f32, Color)]) {
        let Some(sr) = rect.skia() else { return };
        let stops: Vec<GradientStop> = stops.iter().map(|(t, c)| GradientStop::new(*t, c.to_skia())).collect();
        let Some(shader) = LinearGradient::new(
            Point::from_xy(p0.0, p0.1),
            Point::from_xy(p1.0, p1.1),
            stops,
            SpreadMode::Pad,
            Transform::identity(),
        ) else {
            return;
        };
        let p = Paint { shader, anti_alias: self.aa, ..Paint::default() };
        self.pm.fill_rect(sr, &p, Transform::identity(), None);
    }

    // ------------------------------------------------------------------ pixels and images

    /// Blends one pixel (no anti-aliasing). Out-of-range coordinates are ignored.
    pub fn blend_pixel(&mut self, x: i32, y: i32, c: Color) {
        if x < 0 || y < 0 || x >= self.width() as i32 || y >= self.height() as i32 || c.a == 0 {
            return;
        }
        let i = ((y as u32 * self.width() + x as u32) * 4) as usize;
        let d = &mut self.pm.data_mut()[i..i + 4];
        blend(d, c, 255);
    }

    pub fn set_pixel(&mut self, x: i32, y: i32, c: Color) {
        if x < 0 || y < 0 || x >= self.width() as i32 || y >= self.height() as i32 {
            return;
        }
        let i = ((y as u32 * self.width() + x as u32) * 4) as usize;
        let a = c.a as u32;
        self.pm.data_mut()[i..i + 4].copy_from_slice(&[
            (c.r as u32 * a / 255) as u8,
            (c.g as u32 * a / 255) as u8,
            (c.b as u32 * a / 255) as u8,
            c.a,
        ]);
    }

    pub fn get_pixel(&self, x: i32, y: i32) -> Color {
        if x < 0 || y < 0 || x >= self.width() as i32 || y >= self.height() as i32 {
            return Color::TRANSPARENT;
        }
        let p = self.pm.pixel(x as u32, y as u32).unwrap();
        let c = p.demultiply();
        Color::rgba(c.red(), c.green(), c.blue(), c.alpha())
    }

    /// Draws another canvas at (x, y) with `opacity`.
    pub fn draw_canvas(&mut self, src: &Canvas, x: f32, y: f32, opacity: f32) {
        let paint = PixmapPaint { opacity, quality: FilterQuality::Nearest, ..PixmapPaint::default() };
        self.pm.draw_pixmap(0, 0, src.pm.as_ref(), &paint, Transform::from_translate(x, y), None);
    }

    /// Draws `src` scaled into the target rectangle; `smooth = false` is nearest neighbour.
    pub fn draw_canvas_scaled(&mut self, src: &Canvas, dst: R, smooth: bool, opacity: f32) {
        let sx = dst.w / src.width() as f32;
        let sy = dst.h / src.height() as f32;
        let quality = if smooth { FilterQuality::Bilinear } else { FilterQuality::Nearest };
        let paint = PixmapPaint { opacity, quality, ..PixmapPaint::default() };
        let t = Transform::from_row(sx, 0.0, 0.0, sy, dst.x, dst.y);
        self.pm.draw_pixmap(0, 0, src.pm.as_ref(), &paint, t, None);
    }

    /// Draws `src` scaled into a rounded rectangle (album covers).
    pub fn draw_canvas_rounded(&mut self, src: &Canvas, dst: R, radius: f32) {
        let Some(path) = Self::round_rect_path(dst.x, dst.y, dst.w, dst.h, radius) else { return };
        let sx = dst.w / src.width() as f32;
        let sy = dst.h / src.height() as f32;
        let shader = Pattern::new(
            src.pm.as_ref(),
            SpreadMode::Pad,
            FilterQuality::Bilinear,
            1.0,
            Transform::from_row(sx, 0.0, 0.0, sy, dst.x, dst.y),
        );
        let p = Paint { shader, anti_alias: true, ..Paint::default() };
        self.pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
    }

    /// Fills everything outside `keep` (used for masks of arbitrary shapes).
    pub fn clip_to_path(&mut self, path: &Path) {
        let Some(mut mask) = Mask::new(self.width(), self.height()) else { return };
        mask.fill_path(path, FillRule::Winding, self.aa, Transform::identity());
        self.pm.apply_mask(&mask);
    }

    /// Nearest-neighbour integer upscale of the whole canvas.
    pub fn scaled_nearest(&self, factor: u32) -> Canvas {
        let f = factor.max(1);
        let mut out = Canvas::new(self.width() * f, self.height() * f);
        let w = self.width() as usize;
        let src = self.pm.data();
        let ow = out.width() as usize;
        let oh = out.height() as usize;
        let dst = out.pm.data_mut();
        for y in 0..oh {
            let sy = y / f as usize;
            for x in 0..ow {
                let sx = x / f as usize;
                let si = (sy * w + sx) * 4;
                let di = (y * ow + x) * 4;
                dst[di..di + 4].copy_from_slice(&src[si..si + 4]);
            }
        }
        out
    }

    // ------------------------------------------------------------------ text

    /// Width of `text` laid out on one line.
    pub fn text_width(text: &str, font: FontSpec) -> f32 {
        let face = font.face();
        let sf = face.as_scaled(font.scale());
        let mut w = 0.0;
        let mut prev = None;
        for ch in text.chars() {
            let id = face.glyph_id(ch);
            if let Some(p) = prev {
                let k = sf.kern(p, id);
                w += if font.aa { k } else { k.round() };
            }
            w += if font.aa { sf.h_advance(id) } else { crate::fonts::mono_glyph(&font, ch).advance };
            prev = Some(id);
        }
        w
    }

    /// Elides with "…" on the right so the text fits `width` (Qt::ElideRight).
    pub fn elide(text: &str, font: FontSpec, width: f32) -> String {
        if Self::text_width(text, font) <= width {
            return text.to_string();
        }
        let mut chars: Vec<char> = text.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let s: String = chars.iter().collect::<String>().trim_end().to_string() + "…";
            if Self::text_width(&s, font) <= width {
                return s;
            }
        }
        "…".to_string()
    }

    /// Elides in the middle (Qt::ElideMiddle).
    pub fn elide_middle(text: &str, font: FontSpec, width: f32) -> String {
        if Self::text_width(text, font) <= width {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let mut keep = chars.len();
        while keep > 0 {
            keep -= 1;
            let head = keep.div_ceil(2);
            let tail = keep / 2;
            let s: String =
                chars[..head].iter().collect::<String>() + "…" + &chars[chars.len() - tail..].iter().collect::<String>();
            if Self::text_width(&s, font) <= width {
                return s;
            }
        }
        "…".to_string()
    }

    /// Word wrap into at most `max_lines`; the last line is elided when text remains.
    /// Words longer than a line are broken by characters.
    pub fn wrap(text: &str, font: FontSpec, width: f32, max_lines: usize) -> Vec<String> {
        let mut words: Vec<String> = Vec::new();
        for w in text.split_whitespace() {
            let mut piece = String::new();
            for ch in w.chars() {
                piece.push(ch);
                if Self::text_width(&piece, font) > width && piece.chars().count() > 1 {
                    piece.pop();
                    words.push(std::mem::take(&mut piece));
                    piece.push(ch);
                }
            }
            if !piece.is_empty() {
                words.push(piece);
            }
        }
        let mut lines: Vec<String> = Vec::new();
        let mut i = 0;
        while i < words.len() && max_lines > 0 {
            if lines.len() + 1 == max_lines {
                lines.push(Self::elide(&words[i..].join(" "), font, width));
                break;
            }
            let mut line = words[i].clone();
            i += 1;
            while i < words.len() {
                let candidate = format!("{line} {}", words[i]);
                if Self::text_width(&candidate, font) > width {
                    break;
                }
                line = candidate;
                i += 1;
            }
            lines.push(line);
        }
        lines
    }

    /// Draws one line of text with its baseline at `baseline`, starting at `x`. Returns the
    /// advance width.
    pub fn text_at(&mut self, x: f32, baseline: f32, text: &str, font: FontSpec, c: Color) -> f32 {
        if !font.aa {
            return self.text_at_mono(x, baseline, text, font, c);
        }
        let face = font.face();
        let scale = font.scale();
        let sf = face.as_scaled(scale);
        let mut pen = x;
        let mut prev = None;
        let by = baseline.round();
        for ch in text.chars() {
            let id = face.glyph_id(ch);
            if let Some(p) = prev {
                pen += sf.kern(p, id);
            }
            let glyph = id.with_scale_and_position(scale, ab_glyph::point(pen.round(), by));
            if let Some(og) = face.outline_glyph(glyph) {
                let b = og.px_bounds();
                let (w, h) = (self.width() as i32, self.height() as i32);
                let aa = font.aa;
                let data = self.pm.data_mut();
                og.draw(|gx, gy, cov| {
                    let px = b.min.x as i32 + gx as i32;
                    let py = b.min.y as i32 + gy as i32;
                    if px < 0 || py < 0 || px >= w || py >= h {
                        return;
                    }
                    let cov = if aa { cov } else if cov >= 0.5 { 1.0 } else { 0.0 };
                    if cov <= 0.0 {
                        return;
                    }
                    let i = ((py * w + px) * 4) as usize;
                    blend(&mut data[i..i + 4], c, (cov.min(1.0) * 255.0) as u32);
                });
            }
            pen += sf.h_advance(id);
            prev = Some(id);
        }
        pen - x
    }

    /// `text_at` without anti-aliasing: hinted monochrome glyphs on whole pixels.
    fn text_at_mono(&mut self, x: f32, baseline: f32, text: &str, font: FontSpec, c: Color) -> f32 {
        let face = font.face();
        let sf = face.as_scaled(font.scale());
        let mut paint = Paint::default();
        paint.set_color(c.to_skia());
        paint.anti_alias = false;
        let start = x.round();
        let by = baseline.round();
        let mut pen = start;
        let mut prev = None;
        for ch in text.chars() {
            let id = face.glyph_id(ch);
            if let Some(p) = prev {
                pen += sf.kern(p, id).round();
            }
            let g = crate::fonts::mono_glyph(&font, ch);
            if let Some(path) = &g.path {
                self.pm.fill_path(path, &paint, FillRule::Winding, Transform::from_translate(pen, by), None);
            }
            pen += g.advance;
            prev = Some(id);
        }
        pen - start
    }

    /// Draws text aligned inside `rect` (one line, no clipping, like QPainter::drawText).
    /// Newlines split lines; each is aligned separately.
    pub fn text(&mut self, rect: R, align: Align, text: &str, font: FontSpec, c: Color) {
        let lines: Vec<&str> = text.split('\n').collect();
        let lh = font.height();
        let total = lh * lines.len() as f32;
        let top = match align.1 {
            VAlign::Top => rect.y,
            VAlign::Center => rect.y + (rect.h - total) / 2.0,
            VAlign::Bottom => rect.bottom() - total,
        };
        for (i, line) in lines.iter().enumerate() {
            let w = Self::text_width(line, font);
            let x = match align.0 {
                HAlign::Left => rect.x,
                HAlign::Center => rect.x + (rect.w - w) / 2.0,
                HAlign::Right => rect.right() - w,
            };
            self.text_at(x, top + i as f32 * lh + font.ascent(), line, font, c);
        }
    }

    /// Text with its top-left at (x, y) (QPainter::drawText(QRect(x, y, big, big), AlignLeft|AlignTop)).
    pub fn text_tl(&mut self, x: f32, y: f32, text: &str, font: FontSpec, c: Color) -> f32 {
        self.text_at(x, y + font.ascent(), text, font, c)
    }

    /// Word-wrapped text in `rect`, at most `max_lines` lines with `line_height` spacing
    /// (0 = font height); the last line is elided.
    pub fn text_wrapped(&mut self, rect: R, align: Align, text: &str, font: FontSpec, c: Color, max_lines: usize, line_height: f32) {
        let lines = Self::wrap(text, font, rect.w, max_lines.max(1));
        let lh = if line_height > 0.0 { line_height } else { font.height() };
        for (i, line) in lines.iter().enumerate() {
            let w = Self::text_width(line, font);
            let x = match align.0 {
                HAlign::Left => rect.x,
                HAlign::Center => rect.x + (rect.w - w) / 2.0,
                HAlign::Right => rect.right() - w,
            };
            self.text_at(x, rect.y + i as f32 * lh + font.ascent(), line, font, c);
        }
    }

    // ------------------------------------------------------------------ helpers shared by modes

    /// Rounded progress bar: background track, then `value` (0..1) of it filled.
    pub fn bar(&mut self, x: f32, y: f32, w: f32, h: f32, value: f32, fill: Color, bg: Option<Color>) {
        let rad = h / 2.0;
        self.fill_round_rect(x, y, w, h, rad, bg.unwrap_or(Color::hex(0x232837)));
        let v = value.clamp(0.0, 1.0);
        if v > 0.0 {
            let fw = (w * v).max(h.min(w));
            self.fill_round_rect(x, y, fw, h, rad, fill);
        }
    }

    /// Draws the 5×7 pixel font (§17.1); `zoom` is an integer scale. Returns the width drawn.
    pub fn pixel_text(&mut self, x: i32, y: i32, text: &str, c: Color, spacing: i32, zoom: i32) -> i32 {
        crate::pixelfont::draw(self, x, y, text, c, spacing, zoom)
    }

    /// Draws a 12×12 icon (§17.2) scaled by `zoom`; `secondary` defaults to `main` at 45%.
    pub fn icon(&mut self, name: &str, x: i32, y: i32, zoom: i32, main: Color, secondary: Option<Color>) -> bool {
        let Some(rows) = crate::icons::template(name) else { return false };
        let sec = secondary.unwrap_or(main.with_alpha((main.a as f32 * 0.45) as u8));
        for (ry, row) in rows.iter().enumerate() {
            for (rx, ch) in row.chars().enumerate() {
                let col = match ch {
                    '#' => main,
                    '+' => sec,
                    _ => continue,
                };
                for dy in 0..zoom {
                    for dx in 0..zoom {
                        self.blend_pixel(x + rx as i32 * zoom + dx, y + ry as i32 * zoom + dy, col);
                    }
                }
            }
        }
        true
    }
}

/// Source-over blend of a straight colour with extra coverage (0..255) into premultiplied RGBA.
#[inline]
fn blend(d: &mut [u8], c: Color, coverage: u32) {
    let a = c.a as u32 * coverage / 255;
    if a == 0 {
        return;
    }
    let inv = 255 - a;
    d[0] = ((c.r as u32 * a + d[0] as u32 * inv) / 255) as u8;
    d[1] = ((c.g as u32 * a + d[1] as u32 * inv) / 255) as u8;
    d[2] = ((c.b as u32 * a + d[2] as u32 * inv) / 255) as u8;
    d[3] = (a + d[3] as u32 * inv / 255) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_and_measures_text() {
        let mut c = Canvas::device();
        let f = FontSpec::bold(12.0);
        let w = Canvas::text_width("Claude Code", f);
        assert!(w > 50.0 && w < 100.0, "{w}");
        c.text(r(0.0, 0.0, 160.0, 20.0), Align::CENTER, "Привет", f, Color::WHITE);
        let lit = c.to_frame().rgb().iter().filter(|&&b| b > 128).count();
        assert!(lit > 30);
        assert_eq!(Canvas::elide("очень длинная строка текста", f, 60.0).chars().last(), Some('…'));
        assert!(Canvas::wrap("Midnight City by the sea", f, 80.0, 2).len() <= 2);
    }

    #[test]
    fn shapes_land_on_frame() {
        let mut c = Canvas::device();
        c.fill_rect(10.0, 10.0, 5.0, 5.0, Color::rgb(255, 0, 0));
        c.fill_round_rect(30.0, 30.0, 20.0, 10.0, 4.0, Color::rgb(0, 255, 0));
        c.arc(80.0, 60.0, 20.0, 90.0, -270.0, 4.0, Color::WHITE, LineCap::Round);
        let f = c.to_frame();
        assert_eq!(f.pixel(12, 12), [255, 0, 0]);
        assert_eq!(f.pixel(40, 35), [0, 255, 0]);
        assert!(c.icon("clock", 0, 100, 1, Color::WHITE, None));
    }
}
