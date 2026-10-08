//! Системный монитор (`sysmon`). Placeholder until the real implementation lands.

use super::{LiveMode, ModeCx};
use crate::canvas::{r, Align, Canvas};
use crate::color::Color;
use crate::fonts::FontSpec;

pub struct SystemMonitor;

impl SystemMonitor {
    pub fn new() -> Self {
        SystemMonitor
    }
}

impl LiveMode for SystemMonitor {
    fn id(&self) -> &'static str {
        "sysmon"
    }
    fn title(&self) -> &'static str {
        "Системный монитор"
    }
    fn subtitle(&self) -> &'static str {
        "CPU, GPU, память, температуры"
    }
    fn icon(&self) -> &'static str {
        "sysmon"
    }
    fn render(&mut self, cx: &mut ModeCx) {
        let mut c = Canvas::device();
        c.text(r(0.0, 0.0, 160.0, 128.0), Align::CENTER, "Системный монитор", FontSpec::bold(12.0), Color::WHITE);
        cx.publish(c.to_frame());
    }
}
