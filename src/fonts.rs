//! Bundled DejaVu fonts (the device screens are drawn with DejaVu Sans on every platform).

use ab_glyph::{Font as _, FontRef};
use std::sync::OnceLock;

pub static SANS_TTF: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");
pub static SANS_BOLD_TTF: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf");
pub static MONO_TTF: &[u8] = include_bytes!("../assets/fonts/DejaVuSansMono.ttf");
pub static MONO_BOLD_TTF: &[u8] = include_bytes!("../assets/fonts/DejaVuSansMono-Bold.ttf");

fn load(bytes: &'static [u8]) -> FontRef<'static> {
    FontRef::try_from_slice(bytes).expect("bundled font")
}

pub fn face(bold: bool, mono: bool) -> &'static FontRef<'static> {
    static F: OnceLock<[FontRef<'static>; 4]> = OnceLock::new();
    let all = F.get_or_init(|| [load(SANS_TTF), load(SANS_BOLD_TTF), load(MONO_TTF), load(MONO_BOLD_TTF)]);
    &all[(mono as usize) * 2 + bold as usize]
}

/// Font description for canvas text: pixel size is the em size, like `QFont::setPixelSize`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontSpec {
    pub px: f32,
    pub bold: bool,
    pub mono: bool,
    /// anti-aliasing; `false` thresholds coverage for crisp pixel text
    pub aa: bool,
}

impl FontSpec {
    pub const fn sans(px: f32) -> Self {
        FontSpec { px, bold: false, mono: false, aa: true }
    }
    pub const fn bold(px: f32) -> Self {
        FontSpec { px, bold: true, mono: false, aa: true }
    }
    pub const fn mono(px: f32) -> Self {
        FontSpec { px, bold: false, mono: true, aa: true }
    }
    pub const fn with_bold(mut self) -> Self {
        self.bold = true;
        self
    }
    pub const fn no_aa(mut self) -> Self {
        self.aa = false;
        self
    }

    pub fn face(&self) -> &'static FontRef<'static> {
        face(self.bold, self.mono)
    }

    pub fn scale(&self) -> ab_glyph::PxScale {
        let f = self.face();
        let upem = f.units_per_em().unwrap_or(2048.0);
        ab_glyph::PxScale::from(self.px * f.height_unscaled() / upem)
    }

    pub fn ascent(&self) -> f32 {
        use ab_glyph::ScaleFont;
        self.face().as_scaled(self.scale()).ascent()
    }

    pub fn descent(&self) -> f32 {
        use ab_glyph::ScaleFont;
        -self.face().as_scaled(self.scale()).descent()
    }

    /// ascent + descent, like `QFontMetrics::height()`
    pub fn height(&self) -> f32 {
        self.ascent() + self.descent()
    }

    pub fn has_glyph(&self, c: char) -> bool {
        self.face().glyph_id(c).0 != 0
    }
}
