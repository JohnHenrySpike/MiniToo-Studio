//! Tray icon (§16.13). PLACEHOLDER — replaced by the real implementation.

use crate::color::Color;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct TrayState {
    /// colour of the little screen in the icon
    pub screen: Color,
    /// «MiniToo Studio\nКолонка: …\nРежим: …\nClaude: …»
    pub tooltip: String,
    pub claude_mode: bool,
    pub streaming: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    ShowWindow,
    ToggleClaude,
    StopStream,
    Quit,
}

pub struct Tray {}

impl Tray {
    /// `None` if there is no tray on this desktop.
    pub fn spawn(_rt: &tokio::runtime::Handle, _initial: TrayState, _on: Arc<dyn Fn(TrayAction) + Send + Sync>) -> Option<Tray> {
        None
    }

    pub fn update(&self, _state: TrayState) {}
}

/// The drawn icon: dark rounded body `#2b2b33`, a "screen" in `screen` colour, two pixel eyes.
pub fn icon_rgba(screen: Color, size: u32) -> image::RgbaImage {
    let mut img = image::RgbaImage::from_pixel(size, size, image::Rgba([0x2b, 0x2b, 0x33, 255]));
    let m = size / 6;
    for y in m..size - 2 * m {
        for x in m..size - m {
            img.put_pixel(x, y, image::Rgba([screen.r, screen.g, screen.b, 255]));
        }
    }
    img
}
