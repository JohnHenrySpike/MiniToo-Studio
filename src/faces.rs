//! Built-in Claude scenes (§11.3, §17.3). PLACEHOLDER — replaced by the port of ClaudeFaces.cpp.

use crate::claude::ClaudeState;
use crate::frame::Frame;

/// One scene: frames (160×128, upscaled ×4 from the 40×32 grid) and per-frame delays in ms.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub frames: Vec<Frame>,
    pub delays: Vec<u32>,
}

/// Variant ids of a state in table order; the first is "classic".
pub fn variants(state: ClaudeState) -> &'static [&'static str] {
    match state {
        ClaudeState::Working => &["classic", "hammer", "gears", "scroll", "treadmill", "juggle", "progress"],
        ClaudeState::Alerting => &["classic", "wave", "bell", "siren", "sign", "knock"],
        ClaudeState::Chilling => &["classic", "coffee", "fishing", "beach", "cloud", "bath"],
    }
}

/// Short Russian title («Ноутбук», «Кузнец», …).
pub fn variant_title(_state: ClaudeState, variant: &str) -> String {
    variant.to_string()
}

/// The most telling frame, for still previews.
pub fn key_frame(_state: ClaudeState, _variant: &str) -> usize {
    0
}

/// Unknown variant → the classic one.
pub fn generate(_state: ClaudeState, _variant: &str) -> Scene {
    Scene { frames: vec![Frame::solid(0x15, 0x15, 0x24)], delays: vec![200] }
}

/// Draws the alert caption (§11.5) onto every frame: dark strip of 23 px at the bottom, line 1
/// in the accent colour 20% lighter, line 2 white, uppercased, 5×7 pixel font with "…"
/// eliding at 152 px, DejaVu Sans 9 px bold for characters the pixel font lacks.
pub fn draw_caption(frames: &[Frame], line1: &str, line2: &str) -> Vec<Frame> {
    let _ = (line1, line2);
    frames.to_vec()
}
