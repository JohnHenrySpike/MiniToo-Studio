//! Сейчас играет (`nowplaying`). Placeholder until the real implementation lands.

use super::{LiveMode, ModeCx};
use crate::canvas::{r, Align, Canvas};
use crate::color::Color;
use crate::fonts::FontSpec;

pub struct NowPlaying;

impl NowPlaying {
    pub fn new() -> Self {
        NowPlaying
    }
}

impl LiveMode for NowPlaying {
    fn id(&self) -> &'static str {
        "nowplaying"
    }
    fn title(&self) -> &'static str {
        "Сейчас играет"
    }
    fn subtitle(&self) -> &'static str {
        "обложка и трек из любого плеера (MPRIS)"
    }
    fn icon(&self) -> &'static str {
        "music"
    }
    fn render(&mut self, cx: &mut ModeCx) {
        let mut c = Canvas::device();
        c.text(r(0.0, 0.0, 160.0, 128.0), Align::CENTER, "Сейчас играет", FontSpec::bold(12.0), Color::WHITE);
        cx.publish(c.to_frame());
    }
}
