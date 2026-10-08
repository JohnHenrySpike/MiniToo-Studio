//! Часы и погода (`clock`). Placeholder until the real implementation lands.

use super::{LiveMode, ModeCx};
use crate::canvas::{r, Align, Canvas};
use crate::color::Color;
use crate::fonts::FontSpec;

pub struct Clock;

impl Clock {
    pub fn new() -> Self {
        Clock
    }
}

impl LiveMode for Clock {
    fn id(&self) -> &'static str {
        "clock"
    }
    fn title(&self) -> &'static str {
        "Часы и погода"
    }
    fn subtitle(&self) -> &'static str {
        "время, дата, Open-Meteo"
    }
    fn icon(&self) -> &'static str {
        "clock"
    }
    fn render(&mut self, cx: &mut ModeCx) {
        let mut c = Canvas::device();
        c.text(r(0.0, 0.0, 160.0, 128.0), Align::CENTER, "Часы и погода", FontSpec::bold(12.0), Color::WHITE);
        cx.publish(c.to_frame());
    }
}
