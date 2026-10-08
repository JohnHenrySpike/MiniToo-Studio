//! Визуализатор звука (`visualizer`). Placeholder until the real implementation lands.

use super::{LiveMode, ModeCx};
use crate::canvas::{r, Align, Canvas};
use crate::color::Color;
use crate::fonts::FontSpec;

pub struct Visualizer;

impl Visualizer {
    pub fn new() -> Self {
        Visualizer
    }
}

impl LiveMode for Visualizer {
    fn id(&self) -> &'static str {
        "visualizer"
    }
    fn title(&self) -> &'static str {
        "Визуализатор звука"
    }
    fn subtitle(&self) -> &'static str {
        "спектр того, что играет на компьютере"
    }
    fn icon(&self) -> &'static str {
        "wave"
    }
    fn render(&mut self, cx: &mut ModeCx) {
        let mut c = Canvas::device();
        c.text(r(0.0, 0.0, 160.0, 128.0), Align::CENTER, "Визуализатор звука", FontSpec::bold(12.0), Color::WHITE);
        cx.publish(c.to_frame());
    }
}
