//! Live modes (§8): screens that render themselves. The controller owns a [`ModeHost`] with one
//! [`ModeSlot`] per mode and drives it from its single event loop, so modes are plain
//! synchronous state machines:
//!
//! * timers: [`ModeCx::timer`] → later [`LiveMode::on_timer`] with the same token;
//! * async work (HTTP, D-Bus, files): [`ModeCx::spawn`] → [`LiveMode::on_message`] with the
//!   future's boxed result;
//! * output: [`ModeCx::publish`] a frame, [`ModeCx::set_status`], [`ModeCx::device_frames_changed`].
//!
//! Everything scheduled while running is dropped when the mode stops (the slot's epoch moves
//! on), so a stopped mode never gets stale callbacks.

pub mod claudestats;
pub mod clock;
pub mod github;
pub mod nowplaying;
pub mod pomodoro;
pub mod sysmon;
pub mod visualizer;

use crate::claude::Session;
use crate::frame::Frame;
use crate::settings::Settings;
use std::any::Any;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

pub type ModeMsg = Box<dyn Any + Send>;

/// Order of modes in the UI list and their ids (§16.9).
pub const MODE_IDS: [&str; 7] = ["clock", "sysmon", "nowplaying", "pomodoro", "claudestats", "github", "visualizer"];

/// What a mode delivers back to the controller loop.
pub enum ModeEventKind {
    Timer(u64),
    Message(ModeMsg),
}

pub struct ModeEvent {
    pub id: &'static str,
    pub epoch: u64,
    pub kind: ModeEventKind,
}

/// Posts mode events into the controller loop.
pub type ModeSink = Arc<dyn Fn(ModeEvent) + Send + Sync>;

/// Shared services handed to modes.
#[derive(Clone)]
pub struct Services {
    pub rt: tokio::runtime::Handle,
    pub http: reqwest::Client,
    pub sink: ModeSink,
}

/// Output collected while a mode method runs.
#[derive(Default)]
pub struct ModeOutput {
    pub frame: Option<Frame>,
    pub status: Option<String>,
    pub device_changed: bool,
    /// a card shown over everything for `ms` (Pomodoro phase end)
    pub overlay: Option<(Frame, u64)>,
    pub logs: Vec<String>,
}

/// Context of one call into a mode.
pub struct ModeCx<'a> {
    pub id: &'static str,
    pub epoch: u64,
    pub settings: &'a mut Settings,
    pub services: &'a Services,
    /// Claude sessions (most recent first), for the statistics screen.
    pub sessions: &'a [Session],
    pub out: &'a mut ModeOutput,
}

impl ModeCx<'_> {
    pub fn now(&self) -> chrono::DateTime<chrono::Local> {
        chrono::Local::now()
    }

    /// Publishes the current frame. Byte-identical frames are ignored by the host.
    pub fn publish(&mut self, frame: Frame) {
        self.out.frame = Some(frame);
    }

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.out.status = Some(s.into());
    }

    /// "My device frames changed": the controller calls [`LiveMode::device_frames`] again if
    /// this mode is on the device. Modes that do not own the signal get it automatically on
    /// every new published frame.
    pub fn device_frames_changed(&mut self) {
        self.out.device_changed = true;
    }

    /// A card shown over every mode for `ms` milliseconds.
    pub fn overlay(&mut self, frame: Frame, ms: u64) {
        self.out.overlay = Some((frame, ms));
    }

    pub fn log(&mut self, s: impl Into<String>) {
        self.out.logs.push(s.into());
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.services.http
    }

    /// One-shot timer; [`LiveMode::on_timer`] gets `token` if the mode is still running.
    pub fn timer(&mut self, after: Duration, token: u64) {
        let sink = self.services.sink.clone();
        let (id, epoch) = (self.id, self.epoch);
        self.services.rt.spawn(async move {
            tokio::time::sleep(after).await;
            sink(ModeEvent { id, epoch, kind: ModeEventKind::Timer(token) });
        });
    }

    /// Runs a future on the runtime; its result goes to [`LiveMode::on_message`].
    pub fn spawn<F>(&mut self, fut: F)
    where
        F: Future<Output = ModeMsg> + Send + 'static,
    {
        let sink = self.services.sink.clone();
        let (id, epoch) = (self.id, self.epoch);
        self.services.rt.spawn(async move {
            let msg = fut.await;
            sink(ModeEvent { id, epoch, kind: ModeEventKind::Message(msg) });
        });
    }

    /// Runs blocking work on the blocking pool; its result goes to [`LiveMode::on_message`].
    pub fn spawn_blocking<F>(&mut self, f: F)
    where
        F: FnOnce() -> ModeMsg + Send + 'static,
    {
        let sink = self.services.sink.clone();
        let (id, epoch) = (self.id, self.epoch);
        self.services.rt.spawn(async move {
            if let Ok(msg) = tokio::task::spawn_blocking(f).await {
                sink(ModeEvent { id, epoch, kind: ModeEventKind::Message(msg) });
            }
        });
    }

    /// A sender usable from any thread to post messages to this mode while it runs.
    pub fn sender(&self) -> ModeSender {
        ModeSender { id: self.id, epoch: self.epoch, sink: self.services.sink.clone() }
    }
}

#[derive(Clone)]
pub struct ModeSender {
    id: &'static str,
    epoch: u64,
    sink: ModeSink,
}

impl ModeSender {
    pub fn send(&self, msg: ModeMsg) {
        (self.sink)(ModeEvent { id: self.id, epoch: self.epoch, kind: ModeEventKind::Message(msg) });
    }
}

/// A live mode. All methods run on the controller loop and must not block for long.
pub trait LiveMode: Send {
    fn id(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn subtitle(&self) -> &'static str;
    fn icon(&self) -> &'static str;

    /// Streamed mode (visualizer): zstd 3, short tail wait.
    fn streaming(&self) -> bool {
        false
    }

    /// "Minute ahead" modes decide themselves when to call `device_frames_changed`.
    fn owns_device_signal(&self) -> bool {
        false
    }

    /// First acquire: start timers and sources. The host calls `render` right after.
    fn start(&mut self, _cx: &mut ModeCx) {}
    /// Last release.
    fn stop(&mut self, _cx: &mut ModeCx) {}
    /// Draw and `publish` the current frame (also used by "refresh").
    fn render(&mut self, cx: &mut ModeCx);
    fn on_timer(&mut self, _cx: &mut ModeCx, _token: u64) {}
    fn on_message(&mut self, _cx: &mut ModeCx, _msg: ModeMsg) {}
    /// Claude sessions changed (only the statistics mode cares).
    fn on_sessions_changed(&mut self, _cx: &mut ModeCx) {}

    /// What to send to the device: frames played `step` ms apart, looped. `None` = the
    /// current frame alone.
    fn device_frames(&mut self, _cx: &mut ModeCx) -> Option<(Vec<Frame>, u32)> {
        None
    }

    /// Settings / buttons from the UI.
    fn command(&mut self, _cx: &mut ModeCx, _cmd: ModeCommand) {}
    /// State for the settings panel in the UI.
    fn view(&self) -> ModeView {
        ModeView::None
    }
}

// ---------------------------------------------------------------------- UI contract

#[derive(Clone, Debug, PartialEq, Default)]
pub struct City {
    pub name: String,
    pub region: String,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ClockView {
    /// 0 «Небо», 1 «Неон», 2 «Пиксели»
    pub style: i64,
    pub city: Option<City>,
    pub results: Vec<City>,
    pub searching: bool,
    pub search_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct NowPlayingView {
    /// false: no MPRIS player at all
    pub available: bool,
    pub player: String,
    pub artist: String,
    pub title: String,
    pub playing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PomodoroPhase {
    #[default]
    Work,
    Break,
    Long,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct PomodoroView {
    pub phase: PomodoroPhase,
    pub running: bool,
    pub remaining: u32,
    pub cycle: u32,
    pub work_min: u32,
    pub break_min: u32,
    pub long_min: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RunState {
    #[default]
    Loading,
    Passed,
    Failed,
    Running,
    /// no runs / cancelled / skipped
    Neutral,
    Error,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct RepoView {
    /// `owner/repo`
    pub name: String,
    pub state: RunState,
    /// «успешно · CI #123 (main)», an error text, …
    pub detail: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct GithubView {
    pub repos: Vec<RepoView>,
    pub has_token: bool,
    /// error of the last «add» attempt
    pub add_error: Option<String>,
    /// polling interval, seconds
    pub interval: u64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct VisualizerView {
    /// 0 «С пиками», 1 «Зеркальные»
    pub style: i64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum ModeView {
    #[default]
    None,
    Clock(ClockView),
    NowPlaying(NowPlayingView),
    Pomodoro(PomodoroView),
    Github(GithubView),
    Visualizer(VisualizerView),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModeCommand {
    ClockStyle(i64),
    ClockSearch(String),
    ClockPickCity(City),
    ClockClearCity,
    PlayPause,
    Next,
    Previous,
    PomodoroStartPause,
    PomodoroSkip,
    PomodoroReset,
    PomodoroWork(u32),
    PomodoroBreak(u32),
    PomodoroLong(u32),
    GithubAdd(String),
    GithubRemove(String),
    GithubToken(String),
    GithubClearToken,
    VisualizerStyle(i64),
}

// ---------------------------------------------------------------------- host

/// A mode plus the bookkeeping of the shared contract (§8.1).
pub struct ModeSlot {
    pub mode: Box<dyn LiveMode>,
    pub refs: u32,
    pub epoch: u64,
    pub frame: Option<Frame>,
    pub revision: u64,
    pub status: String,
}

/// What the host reports after a call into a mode.
#[derive(Default)]
pub struct HostEffects {
    /// the mode's device frames changed (only meaningful if it is on the device)
    pub device_changed: bool,
    pub frame_changed: bool,
    pub overlay: Option<(Frame, u64)>,
    pub logs: Vec<String>,
}

pub struct ModeHost {
    pub slots: Vec<ModeSlot>,
    pub services: Services,
}

impl ModeHost {
    pub fn new(services: Services) -> Self {
        let modes: Vec<Box<dyn LiveMode>> = vec![
            Box::new(clock::Clock::new()),
            Box::new(sysmon::SystemMonitor::new()),
            Box::new(nowplaying::NowPlaying::new()),
            Box::new(pomodoro::Pomodoro::new()),
            Box::new(claudestats::ClaudeStats::new()),
            Box::new(github::Github::new()),
            Box::new(visualizer::Visualizer::new()),
        ];
        let slots = modes
            .into_iter()
            .map(|mode| ModeSlot { mode, refs: 0, epoch: 1, frame: None, revision: 0, status: String::new() })
            .collect();
        ModeHost { slots, services }
    }

    pub fn index(&self, id: &str) -> Option<usize> {
        self.slots.iter().position(|s| s.mode.id() == id)
    }

    pub fn get(&self, id: &str) -> Option<&ModeSlot> {
        self.slots.iter().find(|s| s.mode.id() == id)
    }

    pub fn ids(&self) -> Vec<&'static str> {
        self.slots.iter().map(|s| s.mode.id()).collect()
    }

    pub fn static_id(&self, id: &str) -> Option<&'static str> {
        self.slots.iter().map(|s| s.mode.id()).find(|m| *m == id)
    }

    fn call<R>(
        &mut self,
        i: usize,
        settings: &mut Settings,
        sessions: &[Session],
        f: impl FnOnce(&mut dyn LiveMode, &mut ModeCx) -> R,
    ) -> (R, HostEffects) {
        let slot = &mut self.slots[i];
        let mut out = ModeOutput::default();
        let r = {
            let mut cx = ModeCx {
                id: slot.mode.id(),
                epoch: slot.epoch,
                settings,
                services: &self.services,
                sessions,
                out: &mut out,
            };
            f(slot.mode.as_mut(), &mut cx)
        };
        let mut fx = HostEffects { overlay: out.overlay, logs: out.logs, ..Default::default() };
        if let Some(s) = out.status {
            slot.status = s;
        }
        if let Some(frame) = out.frame {
            if slot.frame.as_ref() != Some(&frame) {
                slot.frame = Some(frame);
                slot.revision += 1;
                fx.frame_changed = true;
                if !slot.mode.owns_device_signal() {
                    fx.device_changed = true;
                }
            }
        }
        if out.device_changed {
            fx.device_changed = true;
        }
        (r, fx)
    }

    /// Reference counting: the first acquire starts the mode and renders a frame at once.
    pub fn acquire(&mut self, id: &str, settings: &mut Settings, sessions: &[Session]) -> HostEffects {
        let Some(i) = self.index(id) else { return HostEffects::default() };
        self.slots[i].refs += 1;
        if self.slots[i].refs > 1 {
            return HostEffects::default();
        }
        let ((), fx) = self.call(i, settings, sessions, |m, cx| {
            m.start(cx);
            m.render(cx);
        });
        fx
    }

    pub fn release(&mut self, id: &str, settings: &mut Settings, sessions: &[Session]) {
        let Some(i) = self.index(id) else { return };
        let slot = &mut self.slots[i];
        if slot.refs == 0 {
            return;
        }
        slot.refs -= 1;
        if slot.refs == 0 {
            let _ = self.call(i, settings, sessions, |m, cx| m.stop(cx));
            self.slots[i].epoch += 1;
        }
    }

    pub fn running(&self, id: &str) -> bool {
        self.get(id).is_some_and(|s| s.refs > 0)
    }

    /// Delivers a timer or message; stale events (older epoch / stopped mode) are dropped.
    pub fn dispatch(&mut self, ev: ModeEvent, settings: &mut Settings, sessions: &[Session]) -> HostEffects {
        let Some(i) = self.index(ev.id) else { return HostEffects::default() };
        if self.slots[i].epoch != ev.epoch || self.slots[i].refs == 0 {
            return HostEffects::default();
        }
        let ((), fx) = match ev.kind {
            ModeEventKind::Timer(t) => self.call(i, settings, sessions, |m, cx| m.on_timer(cx, t)),
            ModeEventKind::Message(msg) => self.call(i, settings, sessions, |m, cx| m.on_message(cx, msg)),
        };
        fx
    }

    pub fn command(&mut self, id: &str, cmd: ModeCommand, settings: &mut Settings, sessions: &[Session]) -> HostEffects {
        let Some(i) = self.index(id) else { return HostEffects::default() };
        let ((), fx) = self.call(i, settings, sessions, |m, cx| m.command(cx, cmd));
        fx
    }

    pub fn refresh(&mut self, id: &str, settings: &mut Settings, sessions: &[Session]) -> HostEffects {
        let Some(i) = self.index(id) else { return HostEffects::default() };
        if self.slots[i].refs == 0 {
            return HostEffects::default();
        }
        let ((), fx) = self.call(i, settings, sessions, |m, cx| m.render(cx));
        fx
    }

    /// Tells running modes that Claude sessions changed; returns ids with effects.
    pub fn sessions_changed(&mut self, settings: &mut Settings, sessions: &[Session]) -> Vec<(&'static str, HostEffects)> {
        let mut all = Vec::new();
        for i in 0..self.slots.len() {
            if self.slots[i].refs == 0 {
                continue;
            }
            let ((), fx) = self.call(i, settings, sessions, |m, cx| m.on_sessions_changed(cx));
            all.push((self.slots[i].mode.id(), fx));
        }
        all
    }

    /// Frames for the device: the mode's own stretch, or its current frame.
    pub fn device_frames(&mut self, id: &str, settings: &mut Settings, sessions: &[Session]) -> Option<(Vec<Frame>, u32)> {
        let i = self.index(id)?;
        let (own, _fx) = self.call(i, settings, sessions, |m, cx| m.device_frames(cx));
        match own {
            Some((frames, step)) if !frames.is_empty() => Some((frames, step)),
            _ => self.slots[i].frame.clone().map(|f| (vec![f], 1000)),
        }
    }

    pub fn streaming(&self, id: &str) -> bool {
        self.get(id).is_some_and(|s| s.mode.streaming())
    }
}

/// Finds the smallest period `p` so that `frames[i] == frames[i - p]` for all i (§8.2), and
/// returns the first `p` frames.
pub fn shortest_period(frames: Vec<Frame>) -> Vec<Frame> {
    let n = frames.len();
    for p in 1..n {
        if (p..n).all(|i| frames[i] == frames[i - p]) {
            return frames[..p].to_vec();
        }
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period() {
        let a = Frame::solid(1, 0, 0);
        let b = Frame::solid(2, 0, 0);
        assert_eq!(shortest_period(vec![a.clone(), b.clone(), a.clone(), b.clone(), a.clone()]).len(), 2);
        assert_eq!(shortest_period(vec![a.clone(), a.clone(), a.clone()]).len(), 1);
        assert_eq!(shortest_period(vec![a.clone(), b.clone(), b.clone()]).len(), 3);
    }
}
