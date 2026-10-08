//! Pomodoro (`pomodoro`). Placeholder until the real implementation lands.

use super::{LiveMode, ModeCx};
use crate::canvas::{r, Align, Canvas};
use crate::color::Color;
use crate::fonts::FontSpec;

pub struct Pomodoro;

impl Pomodoro {
    pub fn new() -> Self {
        Pomodoro
    }
}

impl LiveMode for Pomodoro {
    fn id(&self) -> &'static str {
        "pomodoro"
    }
    fn title(&self) -> &'static str {
        "Pomodoro"
    }
    fn subtitle(&self) -> &'static str {
        "фокус 25 мин, перерыв 5 мин"
    }
    fn icon(&self) -> &'static str {
        "timer"
    }
    fn render(&mut self, cx: &mut ModeCx) {
        let mut c = Canvas::device();
        c.text(r(0.0, 0.0, 160.0, 128.0), Align::CENTER, "Pomodoro", FontSpec::bold(12.0), Color::WHITE);
        cx.publish(c.to_frame());
    }
}
