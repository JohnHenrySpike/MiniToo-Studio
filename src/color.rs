//! RGBA colour with Qt-like helpers (`lighter`, `darker`), shared by the canvas and the UI.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);
    /// Claude terracotta, the accent of device screens.
    pub const ACCENT: Color = Color::hex(0xd97757);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color { r, g, b, a }
    }

    /// `0xRRGGBB`
    pub const fn hex(v: u32) -> Self {
        Color::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// `0xAARRGGBB` (Qt's `#AARRGGBB` notation)
    pub const fn argb(v: u32) -> Self {
        Color::rgba((v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8)
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Color { a, ..self }
    }

    pub fn with_alpha_f(self, f: f32) -> Self {
        Color { a: (f.clamp(0.0, 1.0) * 255.0).round() as u8, ..self }
    }

    /// Parses `#rrggbb` or `#aarrggbb`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#')?;
        let v = u32::from_str_radix(s, 16).ok()?;
        match s.len() {
            6 => Some(Color::hex(v)),
            8 => Some(Color::argb(v)),
            _ => None,
        }
    }

    pub fn to_hsv(self) -> (f32, f32, f32) {
        let r = self.r as f32 / 255.0;
        let g = self.g as f32 / 255.0;
        let b = self.b as f32 / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d == 0.0 {
            0.0
        } else if max == r {
            ((g - b) / d).rem_euclid(6.0) / 6.0
        } else if max == g {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };
        let s = if max == 0.0 { 0.0 } else { d / max };
        (h, s, max)
    }

    /// h, s, v in 0..1
    pub fn from_hsv(h: f32, s: f32, v: f32) -> Self {
        let h = h.rem_euclid(1.0) * 6.0;
        let i = h.floor() as i32;
        let f = h - i as f32;
        let p = v * (1.0 - s);
        let q = v * (1.0 - s * f);
        let t = v * (1.0 - s * (1.0 - f));
        let (r, g, b) = match i {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        Color::rgb((r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8)
    }

    /// Qt `QColor::lighter(factor)`: `lighter(1.4)` = "40% lighter".
    pub fn lighter(self, factor: f32) -> Self {
        if factor <= 0.0 {
            return self;
        }
        if factor < 1.0 {
            return self.darker(1.0 / factor);
        }
        let (h, mut s, v) = self.to_hsv();
        let mut v = v * 255.0 * factor;
        if v > 255.0 {
            s -= (v - 255.0) / 255.0;
            if s < 0.0 {
                s = 0.0;
            }
            v = 255.0;
        }
        Color { a: self.a, ..Color::from_hsv(h, s, v / 255.0) }
    }

    /// Qt `QColor::darker(factor)`: `darker(3.2)` = "3.2 times darker".
    pub fn darker(self, factor: f32) -> Self {
        if factor <= 0.0 {
            return self;
        }
        if factor < 1.0 {
            return self.lighter(1.0 / factor);
        }
        let (h, s, v) = self.to_hsv();
        Color { a: self.a, ..Color::from_hsv(h, s, v / factor) }
    }

    /// Linear mix: `t = 0` → self, `t = 1` → other.
    pub fn mix(self, other: Color, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color { r: m(self.r, other.r), g: m(self.g, other.g), b: m(self.b, other.b), a: m(self.a, other.a) }
    }

    pub fn to_skia(self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba8(self.r, self.g, self.b, self.a)
    }

    pub fn to_egui(self) -> egui::Color32 {
        egui::Color32::from_rgba_unmultiplied(self.r, self.g, self.b, self.a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qt_like_lighter_darker() {
        let c = Color::hex(0xd97757);
        let d = c.darker(3.2);
        assert!(d.r < 80 && d.r > 60, "{d:?}");
        let l = c.lighter(1.4);
        assert_eq!(l.r, 255);
        assert_eq!(Color::argb(0x40ee6b3d), Color::rgba(0xee, 0x6b, 0x3d, 0x40));
    }
}
