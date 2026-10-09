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

/// A glyph hinted for monochrome rendering. Stems land on whole pixels instead of being cut out
/// of an anti-aliased outline by a threshold.
///
/// The automatic hinter (FreeType's autohinter at the mono target) is used: it gives every
/// letter the same stem widths and aligned heights, so small bold text reads evenly. DejaVu's
/// own TrueType instructions (what Qt's `QFont::NoAntialias` shows) leave stems of one or two
/// pixels within a word. Letters with a diaeresis keep the TrueType hints, since the autohinter
/// merges the two dots into a bar at 12–13 px.
pub struct MonoGlyph {
    /// outline in pixels, origin at the pen position on the baseline, y down
    pub path: Option<tiny_skia::Path>,
    /// hinted advance, whole pixels
    pub advance: f32,
}

mod mono {
    use super::{FontSpec, MONO_BOLD_TTF, MONO_TTF, MonoGlyph, SANS_BOLD_TTF, SANS_TTF, keeps_truetype_hints};
    use skrifa::instance::{LocationRef, Size};
    use skrifa::outline::{DrawSettings, Engine, HintingInstance, HintingOptions, OutlinePen, Target};
    use skrifa::{FontRef, GlyphId, MetadataProvider};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Arc;

    struct Pen(tiny_skia::PathBuilder);

    impl OutlinePen for Pen {
        fn move_to(&mut self, x: f32, y: f32) {
            self.0.move_to(x, -y);
        }
        fn line_to(&mut self, x: f32, y: f32) {
            self.0.line_to(x, -y);
        }
        fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
            self.0.quad_to(cx0, -cy0, x, -y);
        }
        fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
            self.0.cubic_to(cx0, -cy0, cx1, -cy1, x, -y);
        }
        fn close(&mut self) {
            self.0.close();
        }
    }

    type Key = (usize, u32);

    #[derive(Default)]
    struct Cache {
        /// per font and size: (autohinter, TrueType interpreter)
        hinters: HashMap<Key, (Option<HintingInstance>, Option<HintingInstance>)>,
        glyphs: HashMap<(Key, char), Arc<MonoGlyph>>,
    }

    thread_local! {
        static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
    }

    fn bytes(index: usize) -> &'static [u8] {
        [SANS_TTF, SANS_BOLD_TTF, MONO_TTF, MONO_BOLD_TTF][index]
    }

    pub fn glyph(spec: &FontSpec, ch: char) -> Arc<MonoGlyph> {
        let key: Key = ((spec.mono as usize) * 2 + spec.bold as usize, spec.px.to_bits());
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            if let Some(g) = c.glyphs.get(&(key, ch)) {
                return g.clone();
            }
            let font = FontRef::new(bytes(key.0)).expect("bundled font");
            let outlines = font.outline_glyphs();
            let size = Size::new(spec.px);
            let hinters = c.hinters.entry(key).or_insert_with(|| {
                let make = |engine| HintingInstance::new(&outlines, size, LocationRef::default(), HintingOptions { engine, target: Target::Mono }).ok();
                (make(Engine::Auto(None)), make(Engine::Interpreter))
            });
            let hinter = if keeps_truetype_hints(ch) { hinters.1.as_ref().or(hinters.0.as_ref()) } else { hinters.0.as_ref().or(hinters.1.as_ref()) };
            let gid = font.charmap().map(ch).unwrap_or(GlyphId::NOTDEF);
            let linear = font.glyph_metrics(size, LocationRef::default()).advance_width(gid).unwrap_or(0.0);
            let mut pen = Pen(tiny_skia::PathBuilder::new());
            let advance = match (outlines.get(gid), hinter) {
                (Some(outline), Some(h)) => outline.draw(DrawSettings::hinted(h, false), &mut pen).ok().and_then(|m| m.advance_width),
                (Some(outline), None) => outline.draw(DrawSettings::unhinted(size, LocationRef::default()), &mut pen).ok().and(None),
                _ => None,
            }
            .unwrap_or(linear)
            .round();
            let g = Arc::new(MonoGlyph { path: pen.0.finish(), advance });
            c.glyphs.insert((key, ch), g.clone());
            g
        })
    }
}

/// Letters whose marks the autohinter fuses (two dots → one bar).
fn keeps_truetype_hints(ch: char) -> bool {
    matches!(ch, 'ё' | 'Ё' | 'ä' | 'ë' | 'ï' | 'ö' | 'ü' | 'ÿ' | 'Ä' | 'Ë' | 'Ï' | 'Ö' | 'Ü' | 'Ÿ' | 'ї' | 'Ї')
}

pub fn mono_glyph(spec: &FontSpec, ch: char) -> std::sync::Arc<MonoGlyph> {
    mono::glyph(spec, ch)
}
