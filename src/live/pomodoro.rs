//! Pomodoro (`pomodoro`, §8.5): focus / break / long break with a ring, a phase-end card over
//! every mode, and "a minute ahead" device frames.

use super::{LiveMode, ModeCommand, ModeCx, ModeView, PomodoroPhase, PomodoroView};
use crate::canvas::{r, Align, Canvas, LineCap};
use crate::color::Color;
use crate::fonts::FontSpec;
use crate::frame::Frame;
use std::time::{Duration, Instant};

pub struct Pomodoro {
    loaded: bool,
    work: u32,
    brk: u32,
    long: u32,
    phase: PomodoroPhase,
    cycle: u32,
    remaining: u32,
    running: bool,
    /// when the next tick is due (while running)
    due: Instant,
    /// tick timers carry this generation; pausing or restarting invalidates older ones
    generation: u64,
    device_key: String,
    device_last: i64,
}

impl Default for Pomodoro {
    fn default() -> Self {
        Self::new()
    }
}

/// What a tick reports when a phase ends: (finished, next).
type PhaseEnd = (PomodoroPhase, PomodoroPhase);

impl Pomodoro {
    pub fn new() -> Self {
        Pomodoro {
            loaded: false,
            work: 25,
            brk: 5,
            long: 15,
            phase: PomodoroPhase::Work,
            cycle: 1,
            remaining: 25 * 60,
            running: false,
            due: Instant::now(),
            generation: 0,
            device_key: String::new(),
            device_last: 0,
        }
    }

    fn ensure_loaded(&mut self, cx: &ModeCx) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        self.work = cx.settings.int_in("pomodoro/work", 25, 1, 180) as u32;
        self.brk = cx.settings.int_in("pomodoro/break", 5, 1, 60) as u32;
        self.long = cx.settings.int_in("pomodoro/long", 15, 1, 90) as u32;
        if !self.running && self.phase == PomodoroPhase::Work {
            self.remaining = self.work * 60;
        }
    }

    pub fn phase_length(&self) -> u32 {
        match self.phase {
            PomodoroPhase::Work => self.work * 60,
            PomodoroPhase::Break => self.brk * 60,
            PomodoroPhase::Long => self.long * 60,
        }
    }

    fn enter(&mut self, phase: PomodoroPhase) {
        self.phase = phase;
        self.remaining = self.phase_length();
    }

    /// One second of the countdown; the phase switches when it reaches zero.
    pub fn tick(&mut self) -> Option<PhaseEnd> {
        self.remaining = self.remaining.saturating_sub(1);
        if self.remaining > 0 {
            return None;
        }
        let finished = self.phase;
        if finished == PomodoroPhase::Work {
            self.enter(if self.cycle.is_multiple_of(4) { PomodoroPhase::Long } else { PomodoroPhase::Break });
        } else {
            self.cycle += 1;
            self.enter(PomodoroPhase::Work);
        }
        Some((finished, self.phase))
    }

    /// «Пропустить»: the phase ends now.
    pub fn skip(&mut self) -> Option<PhaseEnd> {
        self.remaining = 0;
        self.tick()
    }

    /// «Сброс»: stopped, cycle 1, focus.
    pub fn reset(&mut self) {
        self.running = false;
        self.generation += 1;
        self.cycle = 1;
        self.enter(PomodoroPhase::Work);
    }

    pub fn phase(&self) -> PomodoroPhase {
        self.phase
    }
    pub fn cycle(&self) -> u32 {
        self.cycle
    }
    pub fn remaining(&self) -> u32 {
        self.remaining
    }
    pub fn running(&self) -> bool {
        self.running
    }

    fn label(&self) -> &'static str {
        match self.phase {
            PomodoroPhase::Work => tr!("pomodoro.phase.work"),
            PomodoroPhase::Break => tr!("pomodoro.phase.break"),
            PomodoroPhase::Long => tr!("pomodoro.phase.long"),
        }
    }

    fn status(&self) -> String {
        let time = format!("{}:{:02}", self.remaining / 60, self.remaining % 60);
        if self.running {
            tr!("pomodoro.status", phase = self.label(), time = time)
        } else {
            tr!("pomodoro.status_paused", phase = self.label(), time = time)
        }
    }

    fn schedule(&self, cx: &mut ModeCx) {
        let wait = self.due.saturating_duration_since(Instant::now());
        cx.timer(wait, self.generation);
    }

    fn toggle(&mut self, cx: &mut ModeCx) {
        self.generation += 1;
        self.running = !self.running;
        if self.running {
            self.due = Instant::now() + Duration::from_secs(1);
            self.schedule(cx);
        }
    }

    fn phase_end(&self, cx: &mut ModeCx, (finished, next): PhaseEnd) {
        let rest = finished == PomodoroPhase::Work;
        let title = if rest { tr!("pomodoro.card.break_title") } else { tr!("pomodoro.card.work_title") };
        let body = if rest {
            let min = if next == PomodoroPhase::Long { self.long } else { self.brk };
            trn!("pomodoro.card.break_body", min)
        } else {
            tr!("pomodoro.card.work_body").to_string()
        };
        cx.log(tr!("pomodoro.log.phase_end", title = title));
        let time = crate::i18n::time_hm(&cx.now());
        cx.overlay(crate::notify_card::render("Pomodoro", title, &body, crate::notify_card::IconSource::named("chronometer"), &time), 8000);
    }

    /// The face with `remaining` seconds left in the current phase.
    pub fn frame_at(&self, remaining: i64) -> Frame {
        let work = self.phase == PomodoroPhase::Work;
        let accent = if work { Color::ACCENT } else { Color::rgb(80, 190, 130) };
        let mut c = Canvas::device();
        c.fill(if work { Color::rgb(22, 12, 12) } else { Color::rgb(10, 22, 16) });

        let (cx, cy, rad) = (80.0, 56.0, 46.0);
        c.stroke_circle(cx, cy, rad, 7.0, accent.darker(3.2));
        // QPainter::drawArc: 1/16 degree, truncated
        let len = self.phase_length().max(1) as f64;
        let span16 = -((360.0 * 16.0 * (remaining as f64 / len)) as i64);
        c.arc(cx, cy, rad, 90.0, span16 as f32 / 16.0, 7.0, accent, LineCap::Round);

        let rem = remaining.max(0);
        let time = format!("{:02}:{:02}", rem / 60, rem % 60);
        c.text(r(0.0, 38.0, 160.0, 28.0), Align::CENTER, &time, FontSpec::bold(24.0), Color::WHITE);
        let label = if self.running { self.label().to_string() } else { tr!("pomodoro.label_paused", phase = self.label()) };
        c.text(r(0.0, 64.0, 160.0, 14.0), Align::CENTER, &label, FontSpec::sans(10.0), accent.lighter(1.4));

        // cycle dots
        let done_count = (self.cycle as i64 - 1).rem_euclid(4) + if work { 0 } else { 1 };
        for i in 0..4 {
            let color = if (i as i64) < done_count { accent } else { accent.darker(3.0) };
            c.fill_circle(59.0 + i as f32 * 14.0, 116.0, 4.0, color);
        }
        c.to_frame()
    }

    fn device_key(&self) -> String {
        format!(
            "{:?} {} {} {} {}",
            self.phase,
            self.cycle,
            self.phase_length(),
            self.running,
            if self.running { -1 } else { self.remaining as i64 }
        )
    }

    fn after_change(&mut self, cx: &mut ModeCx) {
        self.render(cx);
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
        tr!("pomodoro.subtitle")
    }
    fn icon(&self) -> &'static str {
        "timer"
    }
    fn owns_device_signal(&self) -> bool {
        true
    }

    fn start(&mut self, cx: &mut ModeCx) {
        self.ensure_loaded(cx);
        if self.running {
            // the timers were dropped while the mode was released: catch up with the clock
            let now = Instant::now();
            let mut ended = None;
            while self.due <= now {
                self.due += Duration::from_secs(1);
                if let Some(end) = self.tick() {
                    ended = Some(end);
                    // the rest of the stretch belongs to the new phase; do not run through it
                    if self.due <= now {
                        let behind = (now - self.due).as_secs() as u32;
                        self.remaining = self.remaining.saturating_sub(behind).max(1);
                        self.due += Duration::from_secs(behind as u64);
                    }
                }
            }
            if let Some(end) = ended {
                self.phase_end(cx, end);
            }
            self.generation += 1;
            self.schedule(cx);
        }
    }

    fn render(&mut self, cx: &mut ModeCx) {
        self.ensure_loaded(cx);
        cx.publish(self.frame_at(self.remaining as i64));
        cx.set_status(self.status());
        // a running timer sends a minute ahead at once; a new batch when the device reaches
        // its last frame
        let key = self.device_key();
        if key == self.device_key && !(self.running && self.remaining as i64 <= self.device_last) {
            return;
        }
        self.device_key = key;
        cx.device_frames_changed();
    }

    fn on_timer(&mut self, cx: &mut ModeCx, token: u64) {
        if token != self.generation || !self.running {
            return;
        }
        self.due += Duration::from_secs(1);
        if let Some(end) = self.tick() {
            self.phase_end(cx, end);
        }
        self.schedule(cx);
        self.after_change(cx);
    }

    fn device_frames(&mut self, cx: &mut ModeCx) -> Option<(Vec<Frame>, u32)> {
        self.ensure_loaded(cx);
        let count = if self.running { self.remaining.min(60) } else { 1 }.max(1);
        let frames: Vec<Frame> = (0..count).map(|i| self.frame_at(self.remaining as i64 - i as i64)).collect();
        self.device_last = self.remaining as i64 - (frames.len() as i64 - 1);
        Some((frames, 1000))
    }

    fn command(&mut self, cx: &mut ModeCx, cmd: ModeCommand) {
        self.ensure_loaded(cx);
        match cmd {
            ModeCommand::PomodoroStartPause => self.toggle(cx),
            ModeCommand::PomodoroSkip => {
                if let Some(end) = self.skip() {
                    self.phase_end(cx, end);
                }
            }
            ModeCommand::PomodoroReset => self.reset(),
            ModeCommand::PomodoroWork(v) => {
                self.work = v.clamp(1, 180);
                cx.settings.set_int("pomodoro/work", self.work as i64);
                if !self.running && self.phase == PomodoroPhase::Work {
                    self.remaining = self.work * 60;
                }
            }
            ModeCommand::PomodoroBreak(v) => {
                self.brk = v.clamp(1, 60);
                cx.settings.set_int("pomodoro/break", self.brk as i64);
            }
            ModeCommand::PomodoroLong(v) => {
                self.long = v.clamp(1, 90);
                cx.settings.set_int("pomodoro/long", self.long as i64);
            }
            _ => return,
        }
        self.after_change(cx);
    }

    fn view(&self) -> ModeView {
        ModeView::Pomodoro(PomodoroView {
            phase: self.phase,
            running: self.running,
            remaining: self.remaining,
            cycle: self.cycle,
            work_min: self.work,
            break_min: self.brk,
            long_min: self.long,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_sequence() {
        let mut p = Pomodoro::new();
        let mut seq = Vec::new();
        for _ in 0..8 {
            let (finished, next) = p.skip().unwrap();
            seq.push((finished, next, p.cycle()));
        }
        use PomodoroPhase::*;
        assert_eq!(
            seq,
            vec![
                (Work, Break, 1),
                (Break, Work, 2),
                (Work, Break, 2),
                (Break, Work, 3),
                (Work, Break, 3),
                (Break, Work, 4),
                (Work, Long, 4),
                (Long, Work, 5),
            ]
        );
        assert_eq!(p.remaining(), 25 * 60);
        p.reset();
        assert_eq!((p.phase(), p.cycle(), p.running()), (Work, 1, false));
    }

    #[test]
    fn countdown_ticks() {
        let mut p = Pomodoro::new();
        assert_eq!(p.tick(), None);
        assert_eq!(p.remaining(), 25 * 60 - 1);
        p.remaining = 1;
        assert_eq!(p.tick(), Some((PomodoroPhase::Work, PomodoroPhase::Break)));
        assert_eq!(p.remaining(), 5 * 60);
        assert_eq!(p.status(), "перерыв 5:00 (пауза)");
    }

    #[test]
    fn ring_and_dots() {
        let mut p = Pomodoro::new();
        let f = p.frame_at(25 * 60);
        // full ring in the accent colour at the top
        assert_eq!(f.pixel(80, 10), [0xd9, 0x77, 0x57]);
        assert_eq!(f.pixel(0, 0), [22, 12, 12]);
        p.skip();
        let f = p.frame_at(60);
        // break: the first dot is done (accent green)
        assert_eq!(f.pixel(59, 116), [80, 190, 130]);
    }

    #[test]
    fn host_flow_and_device_frames() {
        use crate::live::{ModeHost, Services};
        use crate::settings::Settings;
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let services = Services { rt: rt.handle().clone(), http: reqwest::Client::new(), sink: std::sync::Arc::new(|_| {}) };
        let mut host = ModeHost::new(services);
        let mut settings = Settings::memory();
        settings.set_int("pomodoro/work", 2);
        let fx = host.acquire("pomodoro", &mut settings, &[]);
        assert!(fx.device_changed && fx.frame_changed);
        assert_eq!(host.get("pomodoro").unwrap().status, "фокус 2:00 (пауза)");
        // paused: one frame
        assert_eq!(host.device_frames("pomodoro", &mut settings, &[]).unwrap().0.len(), 1);
        let fx = host.command("pomodoro", ModeCommand::PomodoroStartPause, &mut settings, &[]);
        assert!(fx.device_changed);
        let (frames, step) = host.device_frames("pomodoro", &mut settings, &[]).unwrap();
        assert_eq!((frames.len(), step), (60, 1000));
        // skipping the phase shows the card
        let fx = host.command("pomodoro", ModeCommand::PomodoroSkip, &mut settings, &[]);
        assert_eq!(fx.overlay.as_ref().map(|o| o.1), Some(8000));
        assert!(matches!(host.slots[3].mode.view(), ModeView::Pomodoro(PomodoroView { phase: PomodoroPhase::Break, .. })));
        // clock «Пиксели»: the rest of the minute, at most 60 frames
        settings.set_int("clock/style", 2);
        host.acquire("clock", &mut settings, &[]);
        let (frames, _) = host.device_frames("clock", &mut settings, &[]).unwrap();
        assert!(!frames.is_empty() && frames.len() <= 60);
    }
}
