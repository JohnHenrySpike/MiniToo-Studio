//! The application controller (§4 and everything around it). One event loop owns all state —
//! like the Qt main thread did: UI commands, HTTP requests, worker events, mode timers, D-Bus
//! signals and capture events all arrive as [`Msg`]s and are handled one at a time. After each
//! batch a fresh [`Snapshot`] is published for the front-ends.

use crate::api::*;
use crate::claude::{self, ClaudeMonitor, ClaudeState};
use crate::faces;
use crate::frame::{Content, Frame};
use crate::gallery::{Gallery, ItemSettings};
use crate::http::{self, Response};
use crate::live::{ModeEvent, ModeHost, Services, HostEffects};
use crate::media::{self, Animation};
use crate::notify_card;
use crate::platform::{bluez, capture, notifications, screensaver, tray};
use crate::protocol::{self, ColorDepth, Incoming};
use crate::rotation::{Rotation, RotationEffect};
use crate::settings::Settings;
use crate::worker::{DeviceWorker, LinkState, MediaJob, WorkerConfig, WorkerEvent};
use chrono::Local;
use rand::seq::SliceRandom;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

pub const DEFAULT_MAC: &str = "B1:21:81:05:E2:65";
pub const DEFAULT_PORT: u16 = 47800;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimerKind {
    Overlay,
    ClaudeApply,
    StateTest,
    Scene,
    RotationTick,
    ScreenTick,
    ScreenPreview,
    PendingRender,
    GallerySave,
    ClaudeExpire,
    Battery,
}

/// What to do once an image finished loading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AfterLoad {
    Nothing,
    Send(Option<Fit>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderKey {
    fit: Fit,
    pixel_art: bool,
    crop: NRect,
}

pub enum Msg {
    Cmd(Command),
    Http(http::Request, oneshot::Sender<Response>),
    Worker(WorkerEvent),
    Mode(ModeEvent),
    Timer(TimerKind, u64),
    Capture(capture::CaptureEvent),
    Notification(Result<notifications::DesktopNotification, String>),
    ScreenLock(bool),
    Battery(bluez::BatteryInfo),
    AudioConnect(Result<(), String>),
    Discovered(Result<Vec<(String, String)>, String>),
    GalleryThumb(String, Option<Frame>),
    FolderThumb(PathBuf, Option<Frame>),
    ImageLoaded { req: u64, path: PathBuf, result: Result<Arc<Animation>, String>, then: AfterLoad },
    Rendered { req: u64, key: RenderKey, frames: Vec<Frame>, speed: u32, send: bool },
    PortalRestart(Result<(), String>),
    PortalCheck(Vec<String>),
    Tray(tray::TrayAction),
    HttpBound(u16, Result<(), String>),
}

#[derive(Clone, Debug, Default)]
pub struct StartOptions {
    pub connect: bool,
    pub image: Option<PathBuf>,
    pub debug: bool,
    pub with_tray: bool,
    pub screenshot: bool,
}

struct ImageDoc {
    anim: Arc<Animation>,
    path: PathBuf,
    name: String,
    id: String,
    source: SourceImage,
}

struct Faces {
    cache: HashMap<(ClaudeState, String), Arc<faces::Scene>>,
    custom: HashMap<ClaudeState, (Arc<Vec<Frame>>, u32)>,
    current: HashMap<ClaudeState, String>,
    bag: HashMap<ClaudeState, Vec<String>>,
    revision: u64,
}

struct Screen {
    capture: Option<capture::Capture>,
    status: CaptureStatus,
    source_size: Option<(u32, u32)>,
    region: NRect,
    fps: u32,
    crisp: bool,
    quality: i64,
    streaming: bool,
    paused: bool,
    busy: bool,
    last_sent: Option<Frame>,
    since_sent: Instant,
    fps_count: u32,
    fps_clock: Instant,
    actual_fps: f32,
    last_bytes: usize,
    error: Option<String>,
    stale: Vec<String>,
    preview: Option<Arc<image::RgbaImage>>,
    preview_counter: u64,
    seen_counter: u64,
    restore_token: Option<String>,
}

pub struct Controller {
    tx: mpsc::UnboundedSender<Msg>,
    rt: tokio::runtime::Handle,
    core: CoreHandle,
    opts: StartOptions,
    settings: Settings,
    worker: DeviceWorker,
    timers: HashMap<TimerKind, u64>,
    log: VecDeque<String>,
    log_arc: Arc<Vec<String>>,

    // device
    conn: Conn,
    device_info: BTreeMap<String, Value>,
    frames_log: VecDeque<String>,
    inflight: usize,
    mac: String,
    brightness: u8,
    battery: Option<u8>,
    audio_connected: bool,
    bluez_found: bool,
    discovering: bool,
    discovered: Vec<(String, String)>,

    // what is on the device (§4.1)
    mode: DisplayMode,
    active_live: Option<&'static str>,
    overlay_active: bool,
    interrupted: bool,
    away: bool,
    device_content: Option<Content>,
    idle_content: Option<Content>,
    last_manual: Option<Content>,
    live_job: u64,
    live_pending: bool,
    stream_job: u64,
    live_frames_sent: u64,
    last_transfer: String,
    mirror: Anim,
    last_sent_frame: Option<Frame>,

    // live modes
    modes: ModeHost,
    previewing: bool,
    rotation: Rotation,

    // claude
    claude: ClaudeMonitor,
    claude_interrupt: bool,
    alert_caption: bool,
    scene_minutes: u32,
    faces: Faces,
    shown_face: Option<ClaudeState>,
    shown_caption: Option<(String, String)>,
    variant_state: Option<ClaudeState>,
    port: u16,
    listening: bool,
    port_busy: bool,
    hooks_message: Option<(bool, String)>,
    hooks_installed: bool,

    // image + gallery
    image: Option<ImageDoc>,
    fit: Fit,
    pixel_art: bool,
    crop: NRect,
    preview: Anim,
    preview_key: Option<RenderKey>,
    image_req: u64,
    render_req: u64,
    image_error: Option<String>,
    gallery: Gallery,
    gallery_view: Arc<Vec<GalleryItemView>>,
    folder_view: Arc<Vec<FolderItemView>>,
    gallery_changed: bool,
    thumbs_running: bool,
    folder_thumbs_running: bool,

    screen: Screen,

    // notifications, lock
    notify_enabled: bool,
    notify_duration: u32,
    notify_ignore: Vec<String>,
    notify_error: Option<String>,
    notify_guard: Option<notifications::MonitorGuard>,
    away_enabled: bool,
    away_brightness: u8,

    tray: Option<tray::Tray>,
    tray_state: Option<tray::TrayState>,
    /// the catalog in use (`ui/language` may say `auto`)
    language_used: String,
    show_serial: u64,
    quit: bool,
    page: usize,
    theme: Theme,
}

fn avg_delay(delays: &[u32]) -> u32 {
    if delays.is_empty() {
        return 1000;
    }
    ((delays.iter().map(|&d| d as u64).sum::<u64>() / delays.len() as u64) as u32).max(20)
}

fn kb(bytes: usize) -> String {
    format!("{:.1}", bytes as f64 / 1024.0)
}

fn half_size(img: &image::RgbaImage) -> image::RgbaImage {
    // quick box downscale for the capture preview
    let (w, h) = (img.width() / 2, img.height() / 2);
    let mut out = image::RgbaImage::new(w.max(1), h.max(1));
    let src = img.as_raw();
    let sw = img.width() as usize;
    for y in 0..h as usize {
        for x in 0..w as usize {
            let mut acc = [0u32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = ((2 * y + dy) * sw + 2 * x + dx) * 4;
                for c in 0..4 {
                    acc[c] += src[i + c] as u32;
                }
            }
            out.put_pixel(x as u32, y as u32, image::Rgba([(acc[0] / 4) as u8, (acc[1] / 4) as u8, (acc[2] / 4) as u8, (acc[3] / 4) as u8]));
        }
    }
    out
}

impl Controller {
    /// Builds the controller and the shared handle. Call [`Controller::run`] on the runtime.
    pub fn new(rt: tokio::runtime::Handle, settings: Settings, opts: StartOptions) -> (Controller, CoreHandle, mpsc::UnboundedReceiver<Msg>) {
        let (tx, rx) = mpsc::unbounded_channel::<Msg>();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Command>();
        let core = CoreHandle(Arc::new(CoreShared {
            snapshot: parking_lot::RwLock::new(Arc::new(Snapshot::default())),
            tx: cmd_tx,
            repaint: parking_lot::RwLock::new(None),
        }));
        {
            let tx = tx.clone();
            rt.spawn(async move {
                while let Some(c) = cmd_rx.recv().await {
                    if tx.send(Msg::Cmd(c)).is_err() {
                        break;
                    }
                }
            });
        }
        let wtx = tx.clone();
        let mac = crate::transport::normalize_mac(&settings.string("device/mac", DEFAULT_MAC));
        let worker = DeviceWorker::new(
            WorkerConfig {
                address: mac.clone(),
                channel: settings.int_in("device/channel", 1, 1, 30) as u8,
                chunk_delay_ms: settings.int_in("device/chunkDelay", 2, 0, 60) as u64,
                keepalive: settings.int_in("device/keepalive", 60, 0, 600) as u64,
            },
            Arc::new(move |e| {
                let _ = wtx.send(Msg::Worker(e));
            }),
        );
        let mtx = tx.clone();
        let services = Services {
            rt: rt.clone(),
            http: reqwest::Client::builder()
                .user_agent("minitoo-studio")
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
            sink: Arc::new(move |e| {
                let _ = mtx.send(Msg::Mode(e));
            }),
        };
        let modes = ModeHost::new(services);
        let mut rotation = Rotation::new(modes.ids().iter().map(|s| s.to_string()).collect());
        rotation.set_members(&settings.list("live/rotation"));
        rotation.set_interval(settings.int_in("live/rotationInterval", 30, 10, 600) as u32);
        let gallery = Gallery::open(Gallery::default_dir());
        let screen = Screen {
            capture: None,
            status: CaptureStatus::Idle,
            source_size: None,
            region: NRect::FULL,
            fps: settings.int_in("screen/fps", 5, 1, 20) as u32,
            crisp: settings.bool("screen/crisp", false),
            quality: settings.int_in("screen/quality", 1, 0, 2),
            streaming: false,
            paused: false,
            busy: false,
            last_sent: None,
            since_sent: Instant::now(),
            fps_count: 0,
            fps_clock: Instant::now(),
            actual_fps: 0.0,
            last_bytes: 0,
            error: None,
            stale: Vec::new(),
            preview: None,
            preview_counter: 0,
            seen_counter: 0,
            restore_token: settings.opt_string("screen/restoreToken"),
        };
        let port = settings.int_in("claude/port", DEFAULT_PORT as i64, 1024, 65535) as u16;
        let c = Controller {
            tx,
            rt,
            core: core.clone(),
            worker,
            timers: HashMap::new(),
            log: VecDeque::new(),
            log_arc: Arc::new(Vec::new()),
            conn: Conn::Disconnected,
            device_info: BTreeMap::new(),
            frames_log: VecDeque::new(),
            inflight: 0,
            mac,
            brightness: settings.int_in("device/brightness", 80, 0, 100) as u8,
            battery: None,
            audio_connected: false,
            bluez_found: false,
            discovering: false,
            discovered: Vec::new(),
            mode: DisplayMode::Idle,
            active_live: None,
            overlay_active: false,
            interrupted: false,
            away: false,
            device_content: None,
            idle_content: None,
            last_manual: None,
            live_job: 0,
            live_pending: false,
            stream_job: 0,
            live_frames_sent: 0,
            last_transfer: String::new(),
            mirror: Anim::default(),
            last_sent_frame: None,
            modes,
            previewing: false,
            rotation,
            claude: ClaudeMonitor::new(settings.bool("claude/idleAlerts", false)),
            claude_interrupt: settings.bool("claude/interrupt", true),
            alert_caption: settings.bool("claude/alertCaption", true),
            scene_minutes: settings.int_in("claude/sceneMinutes", 5, 0, 120) as u32,
            faces: Faces { cache: HashMap::new(), custom: HashMap::new(), current: HashMap::new(), bag: HashMap::new(), revision: 1 },
            shown_face: None,
            shown_caption: None,
            variant_state: None,
            port,
            listening: false,
            port_busy: false,
            hooks_message: None,
            hooks_installed: false,
            image: None,
            fit: Fit::from_i64(settings.int("image/fitMode", 0)),
            pixel_art: false,
            crop: NRect::FULL,
            preview: Anim::default(),
            preview_key: None,
            image_req: 0,
            render_req: 0,
            image_error: None,
            gallery,
            gallery_view: Arc::new(Vec::new()),
            folder_view: Arc::new(Vec::new()),
            gallery_changed: true,
            thumbs_running: false,
            folder_thumbs_running: false,
            screen,
            notify_enabled: settings.bool("notify/enabled", false),
            notify_duration: settings.int_in("notify/duration", 6, 2, 60) as u32,
            notify_ignore: settings.list("notify/ignore"),
            notify_error: None,
            notify_guard: None,
            away_enabled: settings.bool("away/enabled", true),
            away_brightness: settings.int_in("away/brightness", 15, 0, 100) as u8,
            tray: None,
            tray_state: None,
            language_used: crate::i18n::current().code.clone(),
            show_serial: 0,
            quit: false,
            page: settings.int_in("ui/page", 0, 0, 5) as usize,
            theme: if settings.string("ui/theme", "beige") == "dark" { Theme::Dark } else { Theme::Beige },
            settings,
            opts,
        };
        (c, core, rx)
    }

    /// The event loop. Returns when the app quits.
    pub async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Msg>) {
        self.startup().await;
        self.publish();
        while let Some(msg) = rx.recv().await {
            self.handle(msg);
            // drain what is already queued, then publish once
            while let Ok(m) = rx.try_recv() {
                self.handle(m);
            }
            self.publish();
            if self.quit {
                break;
            }
        }
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if self.gallery.is_dirty() {
            self.gallery.flush();
        }
        if let Some(c) = self.screen.capture.take() {
            c.stop();
        }
        self.notify_guard = None;
        self.worker.shutdown();
    }

    // ------------------------------------------------------------------ timers

    fn start_timer(&mut self, kind: TimerKind, ms: u64) {
        let g = self.timers.entry(kind).or_insert(0);
        *g += 1;
        let generation = *g;
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            let _ = tx.send(Msg::Timer(kind, generation));
        });
    }

    fn stop_timer(&mut self, kind: TimerKind) {
        *self.timers.entry(kind).or_insert(0) += 1;
    }

    fn timer_active(&self, kind: TimerKind, generation: u64) -> bool {
        self.timers.get(&kind) == Some(&generation)
    }

    // ------------------------------------------------------------------ startup (§14.3)

    async fn startup(&mut self) {
        // device link
        self.worker.start();
        // HTTP
        self.start_http(self.port).await;
        if self.opts.connect && self.settings.bool("device/autoConnect", true) {
            self.add_log(tr!("log.connecting", mac = self.mac));
            self.worker.set_want_connected(true);
        }
        // platform services
        let tx = self.tx.clone();
        screensaver::spawn_watch(&self.rt, Arc::new(move |active| {
            let _ = tx.send(Msg::ScreenLock(active));
        }));
        if self.notify_enabled {
            self.start_notifications();
        }
        if self.opts.with_tray {
            self.spawn_tray();
        }
        self.start_timer(TimerKind::Battery, 1000);
        self.start_timer(TimerKind::ClaudeExpire, 10_000);
        self.hooks_installed = claude::hooks_installed(self.port);
        self.load_faces();
        self.request_missing_thumbs();
        self.screen.stale = capture::stale_portal_units();
        // the last picture, Claude mode, live mode / rotation
        if let Some(p) = self.opts.image.clone() {
            self.open_image(p, AfterLoad::Nothing);
        } else if let Some(last) = self.settings.opt_string("image/last") {
            let p = PathBuf::from(last);
            if p.exists() {
                self.open_image(p, AfterLoad::Nothing);
            }
        }
        if self.settings.bool("claude/modeOnStart", false) {
            self.set_claude_mode(true);
        }
        let live = self.settings.opt_string("live/onStart").unwrap_or_default();
        if self.settings.bool("live/rotationOnStart", false) && self.rotation.count() > 0 {
            let fx = self.rotation.start(&live);
            self.apply_rotation(fx);
        } else if !live.is_empty() {
            self.show_live(&live);
        }
    }

    async fn start_http(&mut self, port: u16) {
        let (htx, mut hrx) = mpsc::unbounded_channel::<http::Incoming>();
        match http::serve(port, htx).await {
            Ok(_) => {
                self.listening = true;
                self.port_busy = false;
                let tx = self.tx.clone();
                self.rt.spawn(async move {
                    while let Some((req, reply)) = hrx.recv().await {
                        if tx.send(Msg::Http(req, reply)).is_err() {
                            break;
                        }
                    }
                });
            }
            Err(e) => {
                self.listening = false;
                self.port_busy = true;
                self.add_log(tr!("log.http_port_busy", port = port, error = e));
            }
        }
    }

    fn restart_http(&mut self, port: u16) {
        // the old listener keeps running until the process ends only if it was bound; binding
        // a new port is enough for the hooks, which always use the current one
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let (htx, mut hrx) = mpsc::unbounded_channel::<http::Incoming>();
            match http::serve(port, htx).await {
                Ok(_) => {
                    let _ = tx.send(Msg::HttpBound(port, Ok(())));
                    while let Some((req, reply)) = hrx.recv().await {
                        if tx.send(Msg::Http(req, reply)).is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Msg::HttpBound(port, Err(e.to_string())));
                }
            }
        });
    }

    fn spawn_tray(&mut self) {
        let tx = self.tx.clone();
        let state = self.tray_state_now();
        self.tray = tray::Tray::spawn(&self.rt, state.clone(), Arc::new(move |a| {
            let _ = tx.send(Msg::Tray(a));
        }));
        self.tray_state = Some(state);
    }

    // ------------------------------------------------------------------ dispatch

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Cmd(c) => self.command(c),
            Msg::Http(req, reply) => {
                let resp = self.route(req);
                let _ = reply.send(resp);
            }
            Msg::Worker(e) => self.worker_event(e),
            Msg::Mode(ev) => {
                let id = ev.id;
                let sessions = self.claude.sessions();
                let fx = self.modes.dispatch(ev, &mut self.settings, &sessions);
                self.handle_fx(id, fx);
            }
            Msg::Timer(kind, generation) => {
                if self.timer_active(kind, generation) {
                    self.timer(kind);
                }
            }
            Msg::Capture(e) => self.capture_event(e),
            Msg::Notification(n) => match n {
                Ok(n) => self.on_notification(&n),
                Err(e) => {
                    self.add_log(tr!("log.notifications_error", error = e));
                    self.notify_error = Some(e);
                }
            },
            Msg::ScreenLock(active) => self.set_away(active),
            Msg::Battery(info) => self.on_battery(info),
            Msg::AudioConnect(r) => {
                match r {
                    Ok(()) => self.add_log(tr!("log.audio_connected")),
                    Err(e) => self.add_log(tr!("log.audio_error", error = e)),
                }
                self.start_timer(TimerKind::Battery, 3000);
            }
            Msg::Discovered(r) => {
                self.discovering = false;
                match r {
                    Ok(list) => self.discovered = list,
                    Err(e) => self.add_log(tr!("log.discovery_error", error = e)),
                }
            }
            Msg::GalleryThumb(id, frame) => {
                if let Some(f) = frame {
                    self.gallery.store_thumb(&id, f);
                    self.gallery_changed = true;
                }
            }
            Msg::FolderThumb(path, frame) => {
                if let Some(f) = frame {
                    self.gallery.store_folder_thumb(&path, f);
                    self.gallery_changed = true;
                }
            }
            Msg::ImageLoaded { req, path, result, then } => self.image_loaded(req, path, result, then),
            Msg::Rendered { req, key, frames, speed, send } => self.rendered(req, key, frames, speed, send),
            Msg::PortalRestart(r) => {
                match r {
                    Ok(()) => self.screen.error = None,
                    Err(e) => self.screen.error = Some(tr!("app.portal_restart_failed", error = e)),
                }
                let tx = self.tx.clone();
                self.rt.spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    let stale = tokio::task::spawn_blocking(capture::stale_portal_units).await.unwrap_or_default();
                    let _ = tx.send(Msg::PortalCheck(stale));
                });
            }
            Msg::PortalCheck(stale) => self.screen.stale = stale,
            Msg::Tray(a) => match a {
                tray::TrayAction::ShowWindow => self.show_serial += 1,
                tray::TrayAction::ToggleClaude => {
                    let on = self.mode != DisplayMode::Claude;
                    self.set_claude_mode(on);
                }
                tray::TrayAction::StopStream => self.set_streaming(false),
                tray::TrayAction::Quit => self.quit = true,
            },
            Msg::HttpBound(port, r) => match r {
                Ok(()) => {
                    self.listening = true;
                    self.port_busy = false;
                    self.add_log(tr!("log.http_listening", port = port));
                }
                Err(e) => {
                    self.listening = false;
                    self.port_busy = true;
                    self.add_log(tr!("log.http_port_busy", port = port, error = e));
                }
            },
        }
    }

    fn timer(&mut self, kind: TimerKind) {
        match kind {
            TimerKind::Overlay => {
                self.overlay_active = false;
                self.restore_content();
            }
            TimerKind::ClaudeApply => self.apply_claude_state(),
            TimerKind::StateTest => {
                if self.claude.end_forced_state() {
                    self.sessions_changed();
                }
            }
            TimerKind::Scene => self.next_scene(),
            TimerKind::RotationTick => {
                let fx = self.rotation.tick();
                let switched = matches!(fx, RotationEffect::Switch(_));
                self.apply_rotation(fx);
                if !switched && self.rotation.ticking() {
                    self.start_timer(TimerKind::RotationTick, 1000);
                }
            }
            TimerKind::ScreenTick => {
                self.screen_tick();
                if self.screen.streaming {
                    self.start_timer(TimerKind::ScreenTick, 1000 / self.screen.fps.max(1) as u64);
                }
            }
            TimerKind::ScreenPreview => {
                self.screen_preview();
                if self.screen.capture.is_some() {
                    self.start_timer(TimerKind::ScreenPreview, 100);
                }
            }
            TimerKind::PendingRender => self.render_pending(false),
            TimerKind::GallerySave => {
                self.gallery.flush();
                self.gallery_changed = true;
            }
            TimerKind::ClaudeExpire => {
                if self.claude.expire(Local::now()) {
                    self.sessions_changed();
                }
                self.start_timer(TimerKind::ClaudeExpire, 10_000);
            }
            TimerKind::Battery => {
                let mac = self.mac.clone();
                let tx = self.tx.clone();
                self.rt.spawn(async move {
                    let info = bluez::poll(mac).await;
                    let _ = tx.send(Msg::Battery(info));
                });
                self.start_timer(TimerKind::Battery, 30_000);
            }
        }
    }

    fn apply_formats(&mut self) {
        crate::i18n::set_formats(&self.settings.string("ui/timeFormat", "auto"), &self.settings.string("ui/dateFormat", "auto"));
        self.relocalize();
    }

    /// The language or a date/time format changed: live modes and the Claude scene are drawn
    /// again (the window and the tray read texts on every repaint).
    fn relocalize(&mut self) {
        let sessions = self.claude.sessions();
        for id in self.modes.ids() {
            if self.modes.running(id) {
                let fx = self.modes.refresh(id, &mut self.settings, &sessions);
                self.handle_fx(id, fx);
            }
        }
        if self.mode == DisplayMode::Claude {
            self.apply_claude_state();
        }
    }

    fn handle_fx(&mut self, id: &'static str, fx: HostEffects) {
        for l in fx.logs {
            self.add_log(l);
        }
        if let Some((frame, ms)) = fx.overlay {
            self.show_overlay(frame, ms);
        }
        if fx.device_changed && self.device_live() == Some(id) {
            self.submit_live();
        }
    }

    // ------------------------------------------------------------------ log

    fn add_log(&mut self, line: impl Into<String>) {
        let line = line.into();
        eprintln!("[minitoo] {line}");
        self.log.push_front(format!("{}  {line}", crate::i18n::time_hms(&Local::now())));
        self.log.truncate(200);
        self.log_arc = Arc::new(self.log.iter().cloned().collect());
    }

    // ------------------------------------------------------------------ device

    fn worker_config(&self) -> WorkerConfig {
        WorkerConfig {
            address: self.mac.clone(),
            channel: self.settings.int_in("device/channel", 1, 1, 30) as u8,
            chunk_delay_ms: self.settings.int_in("device/chunkDelay", 2, 0, 60) as u64,
            keepalive: self.settings.int_in("device/keepalive", 60, 0, 600) as u64,
        }
    }

    fn apply_device_config(&mut self) {
        self.worker.configure(self.worker_config());
    }

    fn worker_event(&mut self, e: WorkerEvent) {
        match e {
            WorkerEvent::State(s) => {
                self.conn = match s {
                    LinkState::Disconnected => Conn::Disconnected,
                    LinkState::Connecting => Conn::Connecting,
                    LinkState::Connected => Conn::Connected,
                };
                if s == LinkState::Connected {
                    self.query_device_info();
                }
            }
            WorkerEvent::Log(l) => self.add_log(l),
            WorkerEvent::MediaSent { id, bytes, frames, ms, ok } => self.on_media_sent(id, bytes, frames, ms, ok),
            WorkerEvent::Frame(f) => self.on_frame(f),
        }
    }

    fn command_bytes(&self, cmd: u8, args: Vec<u8>) {
        self.worker.submit_command(cmd, args);
    }

    fn send_json(&self, v: Value) {
        self.command_bytes(0x01, serde_json::to_vec(&v).unwrap_or_default());
    }

    fn query_device_info(&self) {
        for op in [0x09u8, 0x0b, 0x13, 0x76, 0x06, 0x15] {
            self.command_bytes(op, vec![]);
        }
        self.command_bytes(0xbd, vec![0x2b]);
        self.command_bytes(0xbd, vec![0x18]);
    }

    fn send_brightness(&self, v: u8) {
        self.command_bytes(0x74, vec![v]);
        self.send_json(json!({"Brightness": v, "Command": "Channel/SetBrightness"}));
    }

    fn set_info(&mut self, key: &str, v: Value) {
        self.device_info.insert(key.to_string(), v);
    }

    fn on_frame(&mut self, f: Incoming) {
        if f.cmd == 0x8b {
            return; // media flow control
        }
        let byte = |i: usize| f.data.get(i).map(|&b| b as i64).unwrap_or(-1);
        let mut interesting = true;
        match f.cmd {
            0xf7 => {
                self.set_info("heartbeat", json!(protocol::hex(&f.data)));
                interesting = false;
            }
            0x09 => self.set_info("volume", json!(byte(0))),
            0x0b => self.set_info("playing", json!(byte(0) == 1)),
            0x13 => self.set_info("workMode", json!(byte(0))),
            0x15 => self.set_info("sdCard", json!(byte(0))),
            0x06 => self.set_info("level06", json!(byte(0))),
            0x76 => {
                let len = byte(0).max(0) as usize;
                let name = f.data.get(1..(1 + len).min(f.data.len())).map(|s| String::from_utf8_lossy(s).into_owned()).unwrap_or_default();
                self.set_info("name", json!(name));
            }
            0x32 => self.set_info("brightnessAck", json!(byte(0))),
            0x01 => {
                if let Ok(o) = serde_json::from_slice::<Value>(&f.data) {
                    let command = o.get("Command").and_then(|c| c.as_str()).unwrap_or("").to_string();
                    if command.starts_with("Tomato/") {
                        interesting = false;
                    }
                    if !command.is_empty() {
                        self.set_info(&format!("json:{command}"), o);
                    }
                }
            }
            0xbd => {
                if byte(0) == 0x13 {
                    interesting = false; // after every media transfer
                }
                let rest = f.data.get(1..).map(protocol::hex).unwrap_or_default();
                self.set_info(&format!("ext_{:02x}", byte(0).max(0)), json!(rest));
            }
            other => self.set_info(&format!("op_{other:02x}"), json!(protocol::hex(&f.data))),
        }
        if interesting {
            let mut raw = vec![0x01];
            raw.extend_from_slice(&f.raw);
            self.frames_log.push_front(format!("{} {}", crate::i18n::time_hms(&Local::now()), protocol::hex(&f.raw)));
            self.frames_log.truncate(100);
        }
    }

    // ------------------------------------------------------------------ submit (§4.2)

    fn submit(&mut self, frames: Vec<Frame>, speed: u32, level: i32, depth: ColorDepth, streaming: bool) -> u64 {
        if frames.is_empty() {
            return 0;
        }
        self.inflight += 1;
        self.device_content = Some(Content::new(frames.clone(), speed));
        self.last_sent_frame = frames.first().cloned();
        self.mirror = Anim { frames: Arc::new(frames.clone()), speed, revision: self.mirror.revision + 1 };
        self.worker.submit_media(MediaJob { id: 0, frames, speed, level, depth, streaming })
    }

    fn zstd_level(&self) -> i32 {
        self.settings.int_in("device/zstdLevel", 19, 1, 22) as i32
    }

    fn on_media_sent(&mut self, id: u64, bytes: usize, frames: usize, ms: u64, ok: bool) {
        self.inflight = self.inflight.saturating_sub(1);
        if id == self.live_job {
            self.live_job = 0;
            if ok {
                self.live_frames_sent += 1;
                let title = self.device_live().and_then(|m| self.modes.get(m)).map(|s| s.mode.title()).unwrap_or("");
                self.last_transfer = tr!("app.transfer.live", title = title, kb = kb(bytes), ms = ms);
            }
            if self.live_pending && self.device_live().is_some() && !self.overlay_active && !self.interrupted {
                self.submit_live();
            }
        } else if id == self.stream_job {
            self.mark_delivered(bytes, ok);
            if ok {
                self.last_transfer = tr!("app.transfer.stream", kb = kb(bytes), ms = ms);
            }
        } else if ok {
            self.last_transfer = trn!("app.transfer.media", frames, kb = kb(bytes), ms = ms);
            self.add_log(tr!("log.sent", transfer = self.last_transfer));
        }
    }

    fn device_live(&self) -> Option<&'static str> {
        if self.away {
            return self.modes.static_id("clock");
        }
        if self.mode == DisplayMode::Live { self.active_live } else { None }
    }

    fn submit_live(&mut self) {
        let Some(id) = self.device_live() else { return };
        if self.overlay_active || self.interrupted || self.live_job != 0 {
            self.live_pending = true; // taken when the link is free: latest wins
            return;
        }
        self.live_pending = false;
        let sessions = self.claude.sessions();
        let Some((frames, step)) = self.modes.device_frames(id, &mut self.settings, &sessions) else { return };
        let streaming = self.modes.streaming(id);
        self.live_job = self.submit(frames, step, if streaming { 3 } else { 9 }, ColorDepth::Full, streaming);
    }

    // ------------------------------------------------------------------ overlays

    fn show_overlay(&mut self, frame: Frame, ms: u64) {
        self.begin_overlay(ms);
        self.submit(vec![frame], 1000, 9, ColorDepth::Full, false);
    }

    fn begin_overlay(&mut self, ms: u64) {
        self.save_idle_content();
        self.overlay_active = true;
        self.screen.paused = true;
        self.update_rotation_pause();
        self.start_timer(TimerKind::Overlay, ms);
    }

    /// Idle sends nothing of its own: remember what the device showed before it is covered.
    fn save_idle_content(&mut self) {
        if !self.overlay_active && !self.interrupted && !self.away {
            self.idle_content = self.device_content.clone();
        }
    }

    fn restore_content(&mut self) {
        self.update_rotation_pause();
        if self.overlay_active {
            return;
        }
        if self.interrupted {
            self.shown_face = None;
            self.show_face(ClaudeState::Alerting);
            self.shown_face = None;
            return;
        }
        if self.away {
            self.live_pending = false;
            self.submit_live();
            return;
        }
        let level = self.zstd_level();
        match self.mode {
            DisplayMode::Screen => self.set_screen_paused(false),
            DisplayMode::Idle => {
                if let Some(c) = self.idle_content.clone() {
                    self.submit(c.frames, c.speed, level, ColorDepth::Full, false);
                }
            }
            DisplayMode::Image => {
                if let Some(c) = self.last_manual.clone() {
                    self.submit(c.frames, c.speed, level, ColorDepth::Full, false);
                }
            }
            DisplayMode::Claude => {
                self.shown_face = None;
                let s = self.claude.state();
                self.show_face(s);
            }
            DisplayMode::Live => {
                if self.active_live.is_some() {
                    self.live_pending = false;
                    self.submit_live();
                }
            }
        }
    }

    fn update_rotation_pause(&mut self) {
        let paused = self.interrupted || self.overlay_active || self.away;
        if self.rotation.set_paused(paused) && self.rotation.ticking() {
            self.start_timer(TimerKind::RotationTick, 1000);
        }
    }

    // ------------------------------------------------------------------ modes

    fn set_mode(&mut self, m: DisplayMode) {
        if m != DisplayMode::Live {
            if self.rotation.stop() {
                self.settings.set_bool("live/rotationOnStart", false);
            }
            self.stop_timer(TimerKind::RotationTick);
            if let Some(id) = self.active_live.take() {
                let sessions = self.claude.sessions();
                self.modes.release(id, &mut self.settings, &sessions);
                self.live_pending = false;
            }
        }
        let on_start = if m == DisplayMode::Live { self.active_live.unwrap_or("") } else { "" };
        if self.settings.string("live/onStart", "") != on_start {
            self.settings.set_string("live/onStart", on_start);
        }
        if m == self.mode {
            return;
        }
        self.mode = m;
        let claude_on = m == DisplayMode::Claude;
        if self.settings.bool("claude/modeOnStart", false) != claude_on {
            self.settings.set_bool("claude/modeOnStart", claude_on);
        }
        if m != DisplayMode::Claude {
            self.shown_face = None;
            self.variant_state = None;
            self.stop_timer(TimerKind::Scene);
        }
    }

    fn show_live(&mut self, id: &str) {
        let Some(id) = self.modes.static_id(id) else { return };
        if self.rotation.stop() {
            self.settings.set_bool("live/rotationOnStart", false);
        }
        self.stop_timer(TimerKind::RotationTick);
        self.activate_live(id);
        let title = self.modes.get(id).map(|s| s.mode.title()).unwrap_or("");
        self.add_log(tr!("log.mode", title = title));
    }

    fn activate_live(&mut self, id: &'static str) {
        self.set_streaming(false);
        self.interrupted = false;
        if self.active_live != Some(id) {
            let sessions = self.claude.sessions();
            if let Some(old) = self.active_live.take() {
                self.modes.release(old, &mut self.settings, &sessions);
            }
            self.active_live = Some(id);
            let fx = self.modes.acquire(id, &mut self.settings, &sessions);
            self.handle_fx(id, HostEffects { device_changed: false, ..fx });
        }
        self.live_pending = false;
        self.set_mode(DisplayMode::Live);
        self.update_rotation_pause();
        self.submit_live();
    }

    fn stop_live(&mut self) {
        if self.mode == DisplayMode::Live {
            self.set_mode(DisplayMode::Idle);
        }
    }

    fn start_rotation(&mut self) {
        if self.rotation.count() == 0 {
            return;
        }
        if self.rotation.count() > 1 {
            self.add_log(trn!("log.rotation", self.rotation.count(), secs = self.rotation.interval()));
        } else {
            let first = self.rotation.members()[0].clone();
            let title = self.modes.get(&first).map(|s| s.mode.title()).unwrap_or("");
            self.add_log(tr!("log.mode", title = title));
        }
        let cur = if self.mode == DisplayMode::Live { self.active_live.unwrap_or("") } else { "" }.to_string();
        let fx = self.rotation.start(&cur);
        self.settings.set_bool("live/rotationOnStart", true);
        self.apply_rotation(fx);
    }

    fn apply_rotation(&mut self, fx: RotationEffect) {
        match fx {
            RotationEffect::None => {}
            RotationEffect::Switch(id) => {
                if let Some(id) = self.modes.static_id(&id) {
                    self.activate_live(id);
                }
                if self.rotation.running() && !self.settings.bool("live/rotationOnStart", false) {
                    self.settings.set_bool("live/rotationOnStart", true);
                }
                self.stop_timer(TimerKind::RotationTick);
                if self.rotation.ticking() {
                    self.start_timer(TimerKind::RotationTick, 1000);
                }
            }
            RotationEffect::Restart => {
                self.stop_timer(TimerKind::RotationTick);
                if self.rotation.ticking() {
                    self.start_timer(TimerKind::RotationTick, 1000);
                }
            }
            RotationEffect::Stopped => {
                self.stop_timer(TimerKind::RotationTick);
                self.settings.set_bool("live/rotationOnStart", false);
            }
        }
    }

    fn set_previewing(&mut self, on: bool) {
        if on == self.previewing {
            return;
        }
        self.previewing = on;
        let sessions = self.claude.sessions();
        for id in self.modes.ids() {
            if on {
                let fx = self.modes.acquire(id, &mut self.settings, &sessions);
                self.handle_fx(id, fx);
            } else {
                self.modes.release(id, &mut self.settings, &sessions);
            }
        }
    }

    // ------------------------------------------------------------------ claude (§4.3, §11)

    fn sessions_changed(&mut self) {
        self.start_timer(TimerKind::ClaudeApply, 250);
        let sessions = self.claude.sessions();
        for (id, fx) in self.modes.sessions_changed(&mut self.settings, &sessions) {
            self.handle_fx(id, fx);
        }
    }

    fn enabled_variants(&self, state: ClaudeState) -> Vec<String> {
        let off = self.settings.list(&format!("claude/scenesOff/{}", state.id()));
        let all = faces::variants(state);
        let on: Vec<String> = all.iter().filter(|v| !off.iter().any(|o| o == *v)).map(|s| s.to_string()).collect();
        if on.is_empty() { vec![all[0].to_string()] } else { on }
    }

    fn scene(&mut self, state: ClaudeState, variant: &str) -> Arc<faces::Scene> {
        self.faces
            .cache
            .entry((state, variant.to_string()))
            .or_insert_with(|| Arc::new(faces::generate(state, variant)))
            .clone()
    }

    /// Custom GIF (5:4 centre crop, pixel art if small, ≤ 92 frames, delays scaled) or the
    /// current scene.
    fn face(&mut self, state: ClaudeState) -> (Arc<Vec<Frame>>, u32) {
        if let Some((frames, speed)) = self.faces.custom.get(&state) {
            return (frames.clone(), *speed);
        }
        let v = self.faces.current.get(&state).cloned().unwrap_or_else(|| "classic".into());
        let s = self.scene(state, &v);
        (Arc::new(s.frames.clone()), avg_delay(&s.delays))
    }

    fn custom_face_path(&self, state: ClaudeState) -> Option<String> {
        self.settings.opt_string(&format!("claude/face/{}", state.id()))
    }

    fn load_faces(&mut self) {
        for state in ClaudeState::ALL {
            self.faces.custom.remove(&state);
            if let Some(path) = self.custom_face_path(state) {
                match media::load(Path::new(&path), media::MAX_LOAD_FRAMES) {
                    Ok(src) => {
                        let pixel = media::auto_pixel_art(src.width, src.height);
                        let crop = media::default_crop(src.width, src.height);
                        let count = src.frames.len();
                        let keep = count.min(protocol::MAX_FRAMES);
                        let mut frames = Vec::with_capacity(keep);
                        let mut delays = Vec::with_capacity(keep);
                        for i in 0..keep {
                            let k = i * count / keep;
                            frames.push(media::render(&src.frames[k], crop, Fit::Crop, pixel));
                            delays.push(src.delays[k] * count as u32 / keep as u32);
                        }
                        self.faces.custom.insert(state, (Arc::new(frames), avg_delay(&delays)));
                    }
                    Err(e) => self.add_log(tr!("app.open_failed", file = path, error = e)),
                }
            }
            let enabled = self.enabled_variants(state);
            let cur = self.faces.current.get(&state).cloned().unwrap_or_default();
            if !enabled.contains(&cur) {
                self.faces.current.insert(state, enabled[0].clone());
            }
        }
        self.faces.revision += 1;
    }

    fn set_current_variant(&mut self, state: ClaudeState, variant: &str) {
        self.faces.current.insert(state, variant.to_string());
        self.faces.revision += 1;
    }

    /// Shuffle bag: every enabled scene once before repeats, never the same twice in a row.
    fn pick_variant(&mut self, state: ClaudeState) {
        let on = self.enabled_variants(state);
        let current = self.faces.current.get(&state).cloned().unwrap_or_default();
        let bag = self.faces.bag.entry(state).or_default();
        bag.retain(|v| on.contains(v));
        if bag.is_empty() {
            *bag = on.clone();
            if bag.len() > 1 {
                bag.retain(|v| *v != current);
            }
            bag.shuffle(&mut rand::rng());
        }
        let v = bag.remove(0);
        self.set_current_variant(state, &v);
        if self.scene_minutes > 0 {
            self.start_timer(TimerKind::Scene, self.scene_minutes as u64 * 60_000);
        }
    }

    fn next_scene(&mut self) {
        let state = self.claude.state();
        let keep = self.mode != DisplayMode::Claude
            || self.overlay_active
            || Some(state) != self.variant_state
            || state == ClaudeState::Alerting
            || self.enabled_variants(state).len() < 2
            || self.custom_face_path(state).is_some();
        if keep {
            if self.scene_minutes > 0 && self.mode == DisplayMode::Claude {
                self.start_timer(TimerKind::Scene, self.scene_minutes as u64 * 60_000);
            }
            return;
        }
        self.pick_variant(state);
        self.shown_face = None;
        self.show_face(state);
    }

    fn caption_lines(&self) -> Option<(String, String)> {
        if !self.alert_caption {
            return None;
        }
        self.claude.alert_caption()
    }

    fn show_face(&mut self, state: ClaudeState) {
        let caption = if state == ClaudeState::Alerting { self.caption_lines() } else { None };
        if Some(state) == self.shown_face && caption == self.shown_caption {
            return;
        }
        if Some(state) != self.variant_state {
            self.variant_state = Some(state);
            if self.custom_face_path(state).is_none() {
                self.pick_variant(state);
            }
        }
        self.shown_face = Some(state);
        self.shown_caption = caption.clone();
        let (frames, speed) = self.face(state);
        match &caption {
            Some((a, b)) => self.add_log(format!("Claude: {} — {a}: {b}", state.id())),
            None => self.add_log(format!("Claude: {}", state.id())),
        }
        let frames = match &caption {
            Some((a, b)) => faces::draw_caption(&frames, a, b),
            None => frames.as_ref().clone(),
        };
        let level = self.zstd_level();
        self.submit(frames, speed, level, ColorDepth::Full, false);
    }

    /// The scene of this state as an overlay for 10 s.
    fn test_face(&mut self, state: ClaudeState) {
        self.begin_overlay(10_000);
        self.variant_state = Some(state);
        self.shown_face = None;
        self.show_face(state);
        self.shown_face = None;
    }

    fn test_state(&mut self, state: ClaudeState) {
        const TEST_MS: u64 = 10_000;
        self.claude.force_state(state, Local::now());
        self.sessions_changed();
        self.start_timer(TimerKind::StateTest, TEST_MS);
        if self.mode == DisplayMode::Claude && !self.away {
            return; // apply_claude_state shows it
        }
        self.begin_overlay(TEST_MS + 400);
        self.shown_face = None;
        self.variant_state = None;
        self.show_face(state);
        self.shown_face = None;
        self.variant_state = None;
    }

    fn apply_claude_state(&mut self) {
        let state = self.claude.state();
        let claude_mode = self.mode == DisplayMode::Claude;
        if claude_mode && !self.away {
            if !self.overlay_active {
                self.show_face(state);
            }
            return;
        }
        // other modes, and Claude mode behind the lock-screen clock: only an alert interrupts
        if !self.claude_interrupt && !claude_mode {
            return;
        }
        let alerting = state == ClaudeState::Alerting;
        if alerting && !self.interrupted && (self.mode != DisplayMode::Idle || self.away) {
            self.save_idle_content();
            self.interrupted = true;
            self.screen.paused = true;
            self.update_rotation_pause();
            if !self.overlay_active {
                self.shown_face = None;
                self.show_face(state);
                self.shown_face = None;
            }
        } else if alerting && self.interrupted && !self.overlay_active && self.caption_lines() != self.shown_caption {
            self.show_face(state); // another session asks, or asks something else
            self.shown_face = None;
        } else if !alerting && self.interrupted {
            self.interrupted = false;
            self.variant_state = None; // the next alert gets a new scene
            self.restore_content();
        }
    }

    fn set_claude_mode(&mut self, on: bool) {
        if on {
            self.set_streaming(false);
            self.interrupted = false;
            self.set_mode(DisplayMode::Claude);
            self.shown_face = None;
            self.apply_claude_state();
        } else if self.mode == DisplayMode::Claude {
            self.set_mode(DisplayMode::Idle);
        }
    }

    // ------------------------------------------------------------------ screen lock (§12)

    fn set_away(&mut self, away: bool) {
        if away && !self.away_enabled {
            return;
        }
        if away == self.away {
            return;
        }
        let sessions = self.claude.sessions();
        if away {
            self.save_idle_content();
            self.add_log(tr!("log.screen_locked", percent = self.away_brightness));
            self.away = true;
            let fx = self.modes.acquire("clock", &mut self.settings, &sessions);
            self.handle_fx("clock", HostEffects { device_changed: false, ..fx });
            self.screen.paused = true;
            self.send_brightness(self.away_brightness);
            self.update_rotation_pause();
            if !self.overlay_active && !self.interrupted {
                self.live_pending = false;
                self.submit_live();
            }
            self.apply_claude_state();
        } else {
            self.add_log(tr!("log.screen_unlocked"));
            self.away = false;
            self.send_brightness(self.brightness);
            if self.mode == DisplayMode::Claude {
                self.interrupted = false;
            }
            self.live_pending = false;
            self.restore_content();
            self.modes.release("clock", &mut self.settings, &sessions);
            self.apply_claude_state();
        }
    }

    // ------------------------------------------------------------------ notifications (§10)

    fn start_notifications(&mut self) {
        let tx = self.tx.clone();
        self.notify_guard = Some(notifications::spawn_monitor(&self.rt, Arc::new(move |n| {
            let _ = tx.send(Msg::Notification(n));
        })));
    }

    fn on_notification(&mut self, n: &notifications::DesktopNotification) {
        let (app, summary, body) = (n.app.as_str(), n.summary.as_str(), n.body.as_str());
        if !self.notify_enabled || app == "MiniToo Studio" {
            return;
        }
        let app_l = app.to_lowercase();
        if self.notify_ignore.iter().any(|i| !i.trim().is_empty() && app_l.contains(&i.trim().to_lowercase())) {
            return;
        }
        self.add_log(tr!("log.notification", app = app, text = summary));
        let card = notify_card::render(app, summary, body, &n.icon, &crate::i18n::time_hm(&Local::now()));
        self.show_overlay(card, self.notify_duration as u64 * 1000);
    }

    // ------------------------------------------------------------------ battery

    fn on_battery(&mut self, info: bluez::BatteryInfo) {
        self.bluez_found = info.found;
        if info.percent != self.battery || info.audio_connected != self.audio_connected {
            if let Some(p) = info.percent {
                if Some(p) != self.battery {
                    self.add_log(tr!("log.battery", percent = p));
                }
            }
            self.battery = info.percent;
            self.audio_connected = info.audio_connected;
        }
    }

    // ------------------------------------------------------------------ image (§6)

    fn open_image(&mut self, path: PathBuf, then: AfterLoad) {
        self.image_req += 1;
        let req = self.image_req;
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let p = path.clone();
            let result = tokio::task::spawn_blocking(move || media::load(&p, media::MAX_LOAD_FRAMES).map(Arc::new))
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            let _ = tx.send(Msg::ImageLoaded { req, path, result, then });
        });
    }

    fn image_loaded(&mut self, req: u64, path: PathBuf, result: Result<Arc<Animation>, String>, then: AfterLoad) {
        if req != self.image_req {
            return;
        }
        let anim = match result {
            Ok(a) => a,
            Err(e) => {
                let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                self.add_log(tr!("app.open_failed", file = name, error = e));
                self.image_error = Some(tr!("app.open_failed", file = name, error = e));
                return;
            }
        };
        self.image_error = None;
        let abs = std::fs::canonicalize(&path).unwrap_or(path);
        self.settings.set_string("image/last", &abs.to_string_lossy());
        // every opened picture goes to the gallery; a known one comes back with its settings
        let id = self.gallery.add(&abs, Some((anim.width, anim.height, anim.frames.len() as u32))).unwrap_or_default();
        self.gallery_changed = true;
        self.schedule_gallery_save();
        let known = self.gallery.entry(&id).cloned();
        let name = known.as_ref().map(|e| e.name.clone()).unwrap_or_else(|| abs.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        if let Some(s) = known.as_ref().and_then(|e| e.settings) {
            self.pixel_art = s.pixel_art;
            self.crop = s.crop;
            if self.fit != s.fit {
                self.fit = s.fit;
                self.settings.set_int("image/fitMode", s.fit as i64);
            }
        } else {
            self.pixel_art = media::auto_pixel_art(anim.width, anim.height);
            self.crop = media::default_crop(anim.width, anim.height);
        }
        let display: Vec<Arc<image::RgbaImage>> = anim.frames.iter().map(|f| media::display_copy(f, 1024)).collect();
        let source = SourceImage {
            path: abs.clone(),
            name: name.clone(),
            width: anim.width,
            height: anim.height,
            frames: Arc::new(display),
            delays: Arc::new(anim.delays.clone()),
            id: id.clone(),
        };
        self.image = Some(ImageDoc { anim, path: abs, name, id, source });
        self.preview_key = None;
        match then {
            AfterLoad::Nothing => self.render_pending(false),
            AfterLoad::Send(fit) => {
                if let Some(f) = fit {
                    self.set_fit(f);
                }
                self.send_image();
            }
        }
    }

    fn render_key(&self) -> RenderKey {
        RenderKey { fit: self.fit, pixel_art: self.pixel_art, crop: self.crop }
    }

    fn schedule_pending(&mut self) {
        self.start_timer(TimerKind::PendingRender, 60);
    }

    /// Renders the device frames for the current settings in the blocking pool.
    fn render_pending(&mut self, send: bool) {
        let Some(doc) = &self.image else { return };
        let key = self.render_key();
        if send && self.preview_key == Some(key) && !self.preview.frames.is_empty() {
            let frames = self.preview.frames.as_ref().clone();
            let speed = self.preview.speed;
            self.do_send(frames, speed);
            return;
        }
        self.render_req += 1;
        let req = self.render_req;
        let anim = doc.anim.clone();
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let r = tokio::task::spawn_blocking(move || {
                let count = anim.frames.len();
                let keep = count.min(protocol::MAX_FRAMES);
                let frames: Vec<Frame> = (0..keep).map(|i| media::render(&anim.frames[i * count / keep], key.crop, key.fit, key.pixel_art)).collect();
                let speed = ((anim.average_delay() as u64 * count as u64) / keep.max(1) as u64).clamp(1, 0xffff) as u32;
                (frames, speed)
            })
            .await;
            if let Ok((frames, speed)) = r {
                let _ = tx.send(Msg::Rendered { req, key, frames, speed, send });
            }
        });
    }

    fn rendered(&mut self, req: u64, key: RenderKey, frames: Vec<Frame>, speed: u32, send: bool) {
        if req != self.render_req || self.image.is_none() {
            return;
        }
        self.preview = Anim { frames: Arc::new(frames.clone()), speed, revision: self.preview.revision + 1 };
        self.preview_key = Some(key);
        if let Some(doc) = &self.image {
            if !doc.id.is_empty() {
                let id = doc.id.clone();
                self.gallery.update(&id, ItemSettings { fit: key.fit, pixel_art: key.pixel_art, crop: key.crop }, frames.first().cloned());
                self.gallery_changed = true;
                self.schedule_gallery_save();
            }
        }
        if send && key == self.render_key() {
            self.do_send(frames, speed);
        }
    }

    fn schedule_gallery_save(&mut self) {
        if self.gallery.is_dirty() {
            self.start_timer(TimerKind::GallerySave, 700);
        }
    }

    fn set_fit(&mut self, f: Fit) {
        if f == self.fit {
            return;
        }
        self.fit = f;
        self.settings.set_int("image/fitMode", f as i64);
        self.schedule_pending();
    }

    fn send_image(&mut self) {
        if self.image.is_none() {
            return;
        }
        self.render_pending(true);
    }

    fn do_send(&mut self, frames: Vec<Frame>, speed: u32) {
        let Some(doc) = &self.image else { return };
        let (name, path, anim) = (doc.name.clone(), doc.path.clone(), doc.anim.clone());
        self.set_streaming(false);
        self.interrupted = false;
        self.last_manual = Some(Content::new(frames.clone(), speed));
        self.set_mode(DisplayMode::Image);
        self.add_log(trn!("log.sending", frames.len(), name = name));
        let level = self.zstd_level();
        self.submit(frames.clone(), speed, level, ColorDepth::Full, false);
        let mut id = self.image.as_ref().map(|d| d.id.clone()).unwrap_or_default();
        if id.is_empty() || self.gallery.entry(&id).is_none() {
            // removed from the gallery while open: sending brings it back
            id = self.gallery.add(&path, Some((anim.width, anim.height, anim.frames.len() as u32))).unwrap_or_default();
            if let Some(d) = self.image.as_mut() {
                d.id = id.clone();
                d.source.id = id.clone();
            }
            let key = self.render_key();
            self.gallery.update(&id, ItemSettings { fit: key.fit, pixel_art: key.pixel_art, crop: key.crop }, frames.first().cloned());
        }
        self.gallery.mark_sent(&id);
        self.gallery_changed = true;
        self.schedule_gallery_save();
    }

    fn open_files(&mut self, paths: Vec<PathBuf>) {
        let Some(first) = paths.first().cloned() else { return };
        if first.is_dir() || paths.len() > 1 {
            let added = self.gallery.add_paths(&paths);
            self.add_log(tr!("log.gallery_added", count = added.len()));
            self.gallery_changed = true;
            self.schedule_gallery_save();
            self.request_missing_thumbs();
        }
        if !first.is_dir() {
            self.open_image(first, AfterLoad::Nothing);
        }
    }

    fn request_missing_thumbs(&mut self) {
        if self.thumbs_running {
            return;
        }
        let missing = self.gallery.missing_thumbs();
        if missing.is_empty() {
            return;
        }
        self.thumbs_running = true;
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let _ = tokio::task::spawn_blocking(move || {
                for (id, path, settings) in missing {
                    let f = crate::gallery::render_file(&path, settings);
                    if tx.send(Msg::GalleryThumb(id, f)).is_err() {
                        break;
                    }
                }
            })
            .await;
        });
        // allow another pass later (new items)
        let tx2 = self.tx.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let _ = tx2.send(Msg::Cmd(Command::RescanFolder));
        });
    }

    fn request_folder_thumbs(&mut self) {
        if self.folder_thumbs_running {
            return;
        }
        let missing = self.gallery.missing_folder_thumbs();
        if missing.is_empty() {
            return;
        }
        self.folder_thumbs_running = true;
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let _ = tokio::task::spawn_blocking(move || {
                for p in missing {
                    let f = crate::gallery::render_file(&p, None);
                    if tx.send(Msg::FolderThumb(p, f)).is_err() {
                        break;
                    }
                }
            })
            .await;
        });
    }

    // ------------------------------------------------------------------ screen (§7)

    fn start_capture(&mut self, new_source: bool) {
        self.screen.error = None;
        self.screen.stale = capture::stale_portal_units();
        if let Some(c) = self.screen.capture.take() {
            c.stop();
        }
        let token = if new_source { None } else { self.screen.restore_token.clone() };
        let tx = self.tx.clone();
        self.screen.status = CaptureStatus::Selecting;
        self.screen.capture = Some(capture::Capture::start(&self.rt, token, Arc::new(move |e| {
            let _ = tx.send(Msg::Capture(e));
        })));
        self.start_timer(TimerKind::ScreenPreview, 100);
    }

    fn stop_capture(&mut self) {
        self.set_streaming(false);
        if let Some(c) = self.screen.capture.take() {
            c.stop();
        }
        self.screen.status = CaptureStatus::Idle;
        self.screen.preview = None;
        self.stop_timer(TimerKind::ScreenPreview);
    }

    fn capture_event(&mut self, e: capture::CaptureEvent) {
        match e {
            capture::CaptureEvent::Started { width, height, restore_token } => {
                self.screen.status = CaptureStatus::Capturing;
                if let Some(t) = restore_token {
                    self.settings.set_string("screen/restoreToken", &t);
                    self.screen.restore_token = Some(t);
                }
                self.set_source_size(width, height);
            }
            capture::CaptureEvent::Resized { width, height } => self.set_source_size(width, height),
            capture::CaptureEvent::Stopped(err) => {
                if let Some(e) = err {
                    self.screen.error = Some(e);
                    self.screen.stale = capture::stale_portal_units();
                }
                self.screen.capture = None;
                self.screen.status = CaptureStatus::Idle;
                self.screen.preview = None;
                self.set_streaming(false);
            }
        }
    }

    fn set_source_size(&mut self, w: u32, h: u32) {
        if self.screen.source_size == Some((w, h)) {
            return;
        }
        self.screen.source_size = Some((w, h));
        // the region chosen last time, if it was for a source of the same size
        let saved = self.settings.raw("screen/region").and_then(|s| {
            let v: Vec<f64> = s.trim_matches('"').split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.len() == 4).then(|| NRect { x: v[0], y: v[1], w: v[2], h: v[3] })
        });
        let src = self.settings.raw("screen/regionSource").map(|s| s.to_string());
        let same = src.as_deref().is_some_and(|s| {
            let nums: Vec<u32> = s.trim_start_matches("@Size(").trim_end_matches(')').split_whitespace().filter_map(|x| x.parse().ok()).collect();
            nums == vec![w, h]
        });
        self.screen.region = match (saved, same) {
            (Some(r), true) if r.w > 0.0 && r.h > 0.0 => r,
            _ => NRect::center_5x4(w as f64, h as f64),
        };
    }

    fn set_region(&mut self, r: NRect) {
        let r = r.clamped();
        self.screen.region = r;
        self.settings.set_string("screen/region", &format!("{} {} {} {}", r.x, r.y, r.w, r.h));
        if let Some((w, h)) = self.screen.source_size {
            self.settings.set_string("screen/regionSource", &format!("@Size({w} {h})"));
        }
    }

    fn set_streaming(&mut self, on: bool) {
        let on = on && self.screen.capture.is_some() && self.screen.status == CaptureStatus::Capturing;
        if self.screen.streaming == on {
            return;
        }
        self.screen.streaming = on;
        self.screen.busy = false;
        self.screen.last_sent = None;
        if on {
            self.start_timer(TimerKind::ScreenTick, 1000 / self.screen.fps.max(1) as u64);
        } else {
            self.stop_timer(TimerKind::ScreenTick);
            if self.mode == DisplayMode::Screen {
                self.set_mode(DisplayMode::Idle);
            }
        }
    }

    fn set_screen_paused(&mut self, on: bool) {
        if self.screen.paused == on {
            return;
        }
        self.screen.paused = on;
        self.screen.last_sent = None; // resend right away on resume
    }

    fn start_stream(&mut self) {
        if self.screen.capture.is_none() || self.screen.status != CaptureStatus::Capturing {
            return;
        }
        self.interrupted = false;
        self.set_screen_paused(false);
        self.set_mode(DisplayMode::Screen);
        self.set_streaming(true);
    }

    fn screen_preview(&mut self) {
        let Some(c) = &self.screen.capture else { return };
        let Some((counter, img)) = c.latest() else { return };
        if counter == self.screen.seen_counter {
            return;
        }
        self.screen.seen_counter = counter;
        let mut small = img.as_ref().clone();
        while small.width() > 960 {
            small = half_size(&small);
        }
        self.screen.preview = Some(Arc::new(small));
        self.screen.preview_counter += 1;
        let (w, h) = (img.width(), img.height());
        self.set_source_size(w, h);
    }

    fn screen_tick(&mut self) {
        if !self.screen.streaming || self.screen.paused || self.screen.busy {
            return;
        }
        let Some(c) = &self.screen.capture else { return };
        let Some((_, full)) = c.latest() else { return };
        let frame = media::render(&full, self.screen.region, Fit::Crop, self.screen.crisp);
        // static screen: skip identical frames, but refresh every few seconds anyway
        if self.screen.last_sent.as_ref() == Some(&frame) && self.screen.since_sent.elapsed() < Duration::from_secs(5) {
            return;
        }
        self.screen.last_sent = Some(frame.clone());
        self.screen.since_sent = Instant::now();
        self.screen.busy = true;
        let level = self.zstd_level().min(6);
        let depth = ColorDepth::from_quality(self.screen.quality);
        self.stream_job = self.submit(vec![frame], 1000, level, depth, true);
    }

    fn mark_delivered(&mut self, bytes: usize, ok: bool) {
        self.screen.busy = false;
        if !ok {
            return;
        }
        self.screen.last_bytes = bytes;
        self.screen.fps_count += 1;
        let ms = self.screen.fps_clock.elapsed().as_millis();
        if ms >= 2000 {
            self.screen.actual_fps = self.screen.fps_count as f32 * 1000.0 / ms as f32;
            self.screen.fps_count = 0;
            self.screen.fps_clock = Instant::now();
        }
    }

    // ------------------------------------------------------------------ commands

    fn command(&mut self, c: Command) {
        match c {
            Command::SetPage(p) => {
                self.page = p.min(5);
                self.settings.set_int("ui/page", self.page as i64);
            }
            Command::SetTheme(t) => {
                self.theme = t;
                self.settings.set_string("ui/theme", if t == Theme::Dark { "dark" } else { "beige" });
            }
            Command::ShowWindow => self.show_serial += 1,
            Command::Quit => self.quit = true,
            Command::PreviewModes(on) => self.set_previewing(on),
            Command::PreviewClaude(_) => {}

            Command::Connect(on) => {
                if on {
                    self.add_log(tr!("log.connecting", mac = self.mac));
                    self.worker.set_want_connected(true);
                } else {
                    self.set_streaming(false);
                    self.worker.set_want_connected(false);
                }
            }
            Command::SetBrightness(v) => {
                let v = v.min(100);
                if v != self.brightness {
                    self.brightness = v;
                    self.settings.set_int("device/brightness", v as i64);
                    if !self.away {
                        self.send_brightness(v);
                    }
                }
            }
            Command::SetVolume(v) => {
                let v = v.min(15);
                self.command_bytes(0x08, vec![v]);
                self.command_bytes(0x09, vec![]);
                self.set_info("volume", json!(v));
            }
            Command::PlayPause => {
                let playing = self.device_info.get("playing").and_then(|v| v.as_bool()).unwrap_or(false);
                self.command_bytes(0x0a, vec![if playing { 0 } else { 1 }]);
                self.command_bytes(0x0b, vec![]);
            }
            Command::PrevTrack => self.command_bytes(0x12, vec![0]),
            Command::NextTrack => self.command_bytes(0x12, vec![1]),
            Command::ScreenOnOff(on) => {
                self.send_json(json!({"Command": "Channel/OnOffScreen", "OnOff": if on { 1 } else { 0 }}));
                self.set_info("screenOn", json!(on));
            }
            Command::SyncTime => {
                self.command_bytes(0x18, protocol::time_args(&Local::now().naive_local()));
                self.add_log(tr!("log.time_synced"));
            }
            Command::Builtin(b) => match b {
                Builtin::Cosmonaut => self.send_json(json!({"Command": "Lyric/Enter"})),
                Builtin::Gallery => self.send_json(json!({"Command": "Photo/Enter"})),
                Builtin::NoiseMeter => self.command_bytes(0x72, vec![0x02, 0x01, 0, 0, 0, 0]),
                Builtin::Tetris => self.command_bytes(0xa0, vec![0x01, 0x00]),
                Builtin::Game2 => self.command_bytes(0xa0, vec![0x01, 0x01]),
                Builtin::Game3 => self.command_bytes(0xa0, vec![0x01, 0x05]),
                Builtin::ExitGame => self.command_bytes(0xa0, vec![0x00, 0x00]),
            },
            Command::Scoreboard { red, blue } => {
                let (r, b) = (red.min(999).to_le_bytes(), blue.min(999).to_le_bytes());
                self.command_bytes(0x72, vec![0x01, 0x01, r[0], r[1], b[0], b[1]]);
            }
            Command::DeviceNotifyCard { app, text } => {
                self.add_log(tr!("log.notification", app = app, text = text));
                let card = notify_card::render(&app, "", &text, &app.to_lowercase(), &crate::i18n::time_hm(&Local::now()));
                self.show_overlay(card, self.notify_duration as u64 * 1000);
            }
            Command::DeviceNotifyIcon { app } => {
                let icon = protocol::NOTIFY_APPS.iter().find(|(n, _)| *n == app).map(|(_, c)| *c).unwrap_or(13);
                let text: Vec<u8> = app.as_bytes().iter().copied().take(128).collect();
                let mut args = vec![icon, text.len() as u8];
                args.extend(text);
                self.command_bytes(0x50, args);
            }
            Command::RawHex(text) => match protocol::parse_hex(&text) {
                Some((cmd, args)) => {
                    let mut all = vec![cmd];
                    all.extend(&args);
                    self.add_log(format!("→ {}", protocol::hex(&all)));
                    self.command_bytes(cmd, args);
                }
                None => self.add_log(tr!("log.bad_hex", text = text)),
            },
            Command::ConnectAudio => {
                if !self.bluez_found {
                    self.add_log(tr!("log.bluez_not_found"));
                    return;
                }
                self.add_log(tr!("log.connecting_audio"));
                let (mac, tx) = (self.mac.clone(), self.tx.clone());
                self.rt.spawn(async move {
                    let r = bluez::connect_audio(mac).await;
                    let _ = tx.send(Msg::AudioConnect(r));
                });
            }
            Command::Discover => {
                if self.discovering {
                    return;
                }
                self.discovering = true;
                self.discovered.clear();
                let tx = self.tx.clone();
                self.rt.spawn(async move {
                    let r = bluez::discover(15).await;
                    let _ = tx.send(Msg::Discovered(r));
                });
            }
            Command::SetAwayEnabled(v) => {
                if v != self.away_enabled {
                    self.away_enabled = v;
                    self.settings.set_bool("away/enabled", v);
                    if !v {
                        self.set_away(false);
                    }
                }
            }
            Command::SetAwayBrightness(v) => {
                let v = v.min(100);
                if v != self.away_brightness {
                    self.away_brightness = v;
                    self.settings.set_int("away/brightness", v as i64);
                    if self.away {
                        self.send_brightness(v);
                    }
                }
            }

            Command::SetMac(m) => {
                let mac = crate::transport::normalize_mac(&m);
                if mac != self.mac {
                    self.mac = mac.clone();
                    self.settings.set_string("device/mac", &mac);
                    self.apply_device_config();
                }
            }
            Command::SetChannel(c) => {
                self.settings.set_int("device/channel", c.clamp(1, 30) as i64);
                self.apply_device_config();
            }
            Command::SetAutoConnect(v) => self.settings.set_bool("device/autoConnect", v),
            Command::SetKeepalive(v) => {
                self.settings.set_int("device/keepalive", v.min(600) as i64);
                self.apply_device_config();
            }
            Command::SetChunkDelay(v) => {
                self.settings.set_int("device/chunkDelay", v.min(60) as i64);
                self.apply_device_config();
            }
            Command::SetZstdLevel(v) => self.settings.set_int("device/zstdLevel", v.clamp(1, 22) as i64),
            Command::SetCloseToTray(v) => self.settings.set_bool("ui/closeToTray", v),
            Command::SetStartHidden(v) => self.settings.set_bool("ui/startHidden", v),
            Command::SetLanguage(code) => {
                let code = if code.trim().is_empty() { "auto".to_string() } else { code.trim().to_string() };
                self.settings.set_string("ui/language", &code);
                self.language_used = crate::i18n::set_language(&code);
                self.relocalize();
            }
            Command::SetTimeFormat(v) => {
                self.settings.set_string("ui/timeFormat", &v);
                self.apply_formats();
            }
            Command::SetDateFormat(v) => {
                let v = if v.trim().is_empty() { "auto".to_string() } else { v };
                self.settings.set_string("ui/dateFormat", &v);
                self.apply_formats();
            }

            Command::OpenFiles(paths) => self.open_files(paths),
            Command::AddPaths(paths) => {
                let single_file = paths.len() == 1 && paths[0].is_file();
                let added = self.gallery.add_paths(&paths);
                if !single_file {
                    self.add_log(tr!("log.gallery_added", count = added.len()));
                }
                self.gallery_changed = true;
                self.schedule_gallery_save();
                self.request_missing_thumbs();
                if single_file {
                    self.open_image(paths[0].clone(), AfterLoad::Nothing);
                }
            }
            Command::SetFit(f) => self.set_fit(f),
            Command::SetPixelArt(v) => {
                if v != self.pixel_art {
                    self.pixel_art = v;
                    self.schedule_pending();
                }
            }
            Command::SetCrop(r) => {
                let r = r.clamped();
                if r != self.crop && r.w > 0.0 && r.h > 0.0 {
                    self.crop = r;
                    self.schedule_pending();
                }
            }
            Command::ResetCrop => {
                if let Some(d) = &self.image {
                    self.crop = media::default_crop(d.anim.width, d.anim.height);
                    self.schedule_pending();
                }
            }
            Command::SendImage => self.send_image(),
            Command::GalleryOpen(id) => {
                if let Some(p) = self.gallery.file_path(&id) {
                    self.open_image(p, AfterLoad::Nothing);
                }
            }
            Command::GallerySend(id) => {
                if self.image.as_ref().is_some_and(|d| d.id == id) {
                    self.send_image();
                } else if let Some(p) = self.gallery.file_path(&id) {
                    self.open_image(p, AfterLoad::Send(None));
                }
            }
            Command::GalleryFavorite(id, on) => {
                self.gallery.set_favorite(&id, on);
                self.gallery_changed = true;
                self.schedule_gallery_save();
            }
            Command::GalleryRemove(id) => {
                self.gallery.remove(&id);
                if let Some(d) = self.image.as_mut() {
                    if d.id == id {
                        d.id.clear();
                        d.source.id.clear();
                    }
                }
                self.gallery_changed = true;
                self.schedule_gallery_save();
            }
            Command::GalleryFilter(f) => {
                self.gallery.filter = f;
                if f == GalleryFilter::Folder {
                    self.gallery.rescan_folder();
                    self.request_folder_thumbs();
                }
                self.gallery_changed = true;
            }
            Command::SetFolder(p) => {
                self.gallery.set_folder(p);
                self.gallery.filter = if self.gallery.folder().is_some() { GalleryFilter::Folder } else { GalleryFilter::All };
                self.gallery_changed = true;
                self.schedule_gallery_save();
                self.request_folder_thumbs();
            }
            Command::RescanFolder => {
                self.thumbs_running = false;
                self.folder_thumbs_running = false;
                if self.gallery.filter == GalleryFilter::Folder {
                    self.gallery.rescan_folder();
                    self.request_folder_thumbs();
                }
                self.request_missing_thumbs_quiet();
                self.gallery_changed = true;
            }
            Command::FolderOpen(p) => self.open_image(p, AfterLoad::Nothing),
            Command::FolderSend(p) => self.open_image(p, AfterLoad::Send(None)),

            Command::SelectSource => self.start_capture(true),
            Command::StartCapture => self.start_capture(false),
            Command::StopCapture => self.stop_capture(),
            Command::SetRegion(r) => self.set_region(r),
            Command::StartStream => self.start_stream(),
            Command::StopStream => self.set_streaming(false),
            Command::SetFps(f) => {
                let f = f.clamp(1, 20);
                self.screen.fps = f;
                self.settings.set_int("screen/fps", f as i64);
            }
            Command::SetCrisp(v) => {
                self.screen.crisp = v;
                self.settings.set_bool("screen/crisp", v);
            }
            Command::SetQuality(q) => {
                let q = q.clamp(0, 2);
                self.screen.quality = q;
                self.settings.set_int("screen/quality", q);
            }
            Command::RestartPortals => {
                let units = self.screen.stale.clone();
                if units.is_empty() {
                    return;
                }
                self.stop_capture();
                let tx = self.tx.clone();
                self.rt.spawn(async move {
                    let r = capture::restart_units(units).await;
                    let _ = tx.send(Msg::PortalRestart(r));
                });
            }

            Command::ShowLive(id) => self.show_live(&id),
            Command::StopLive => self.stop_live(),
            Command::Mode(id, cmd) => {
                if let Some(id) = self.modes.static_id(&id) {
                    let sessions = self.claude.sessions();
                    let fx = self.modes.command(id, cmd, &mut self.settings, &sessions);
                    self.handle_fx(id, fx);
                }
            }
            Command::SetRotationMember(id, on) => {
                let fx = self.rotation.set_member(&id, on);
                self.settings.set_list("live/rotation", self.rotation.members());
                self.apply_rotation(fx);
            }
            Command::StartRotation => self.start_rotation(),
            Command::StopRotation => {
                if self.rotation.stop() {
                    self.settings.set_bool("live/rotationOnStart", false);
                }
                self.stop_timer(TimerKind::RotationTick);
            }
            Command::ClearLog => {
                self.log.clear();
                self.log_arc = Arc::new(Vec::new());
            }
            Command::RotationNext => {
                let fx = self.rotation.next();
                self.apply_rotation(fx);
            }
            Command::SetRotationInterval(s) => {
                let fx = self.rotation.set_interval(s);
                self.settings.set_int("live/rotationInterval", self.rotation.interval() as i64);
                self.apply_rotation(fx);
            }
            Command::SetNotifyEnabled(on) => {
                if on != self.notify_enabled {
                    self.notify_enabled = on;
                    self.settings.set_bool("notify/enabled", on);
                    self.notify_error = None;
                    if on {
                        self.start_notifications();
                    } else {
                        self.notify_guard = None;
                    }
                }
            }
            Command::SetNotifyDuration(s) => {
                self.notify_duration = s.clamp(2, 60);
                self.settings.set_int("notify/duration", self.notify_duration as i64);
            }
            Command::SetNotifyIgnore(list) => {
                self.notify_ignore = list.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                self.settings.set_list("notify/ignore", &self.notify_ignore);
            }
            Command::TestNotification => {
                let was = self.notify_enabled;
                self.notify_enabled = true;
                self.on_notification(&notifications::DesktopNotification {
                    app: "Telegram".into(),
                    summary: tr!("app.test_notification.summary").into(),
                    body: tr!("app.test_notification.body").into(),
                    ..Default::default()
                });
                self.notify_enabled = was;
            }

            Command::SetClaudeMode(on) => self.set_claude_mode(on),
            Command::SetInterrupt(v) => {
                self.claude_interrupt = v;
                self.settings.set_bool("claude/interrupt", v);
            }
            Command::SetIdleAlerts(v) => {
                self.claude.idle_alerts = v;
                self.settings.set_bool("claude/idleAlerts", v);
            }
            Command::SetAlertCaption(v) => {
                self.alert_caption = v;
                self.settings.set_bool("claude/alertCaption", v);
                self.start_timer(TimerKind::ClaudeApply, 250);
            }
            Command::TestState(s) => self.test_state(s),
            Command::ShowStateOnDevice(s) => self.test_face(s),
            Command::PickScene(s, v) => {
                self.set_current_variant(s, &v);
                self.variant_state = Some(s);
                self.test_face(s);
            }
            Command::SetSceneEnabled(s, v, on) => {
                let key = format!("claude/scenesOff/{}", s.id());
                let mut off = self.settings.list(&key);
                if on {
                    off.retain(|x| *x != v);
                } else if !off.contains(&v) {
                    off.push(v.clone());
                }
                if off.len() >= faces::variants(s).len() {
                    return; // keep at least one
                }
                self.settings.set_list(&key, &off);
                if !on && self.faces.current.get(&s) == Some(&v) {
                    self.pick_variant(s);
                    if self.shown_face == Some(s) {
                        self.shown_face = None;
                        self.apply_claude_state();
                    }
                }
                self.faces.revision += 1;
            }
            Command::SetCustomFace(s, path) => {
                let key = format!("claude/face/{}", s.id());
                match path {
                    Some(p) => {
                        if media::load(&p, 1).is_err() {
                            self.add_log(tr!("app.open_failed_file", file = p.display()));
                            return;
                        }
                        self.settings.set_string(&key, &p.to_string_lossy());
                    }
                    None => self.settings.remove(&key),
                }
                self.load_faces();
                if self.shown_face == Some(s) {
                    self.shown_face = None;
                    self.apply_claude_state();
                }
            }
            Command::SetSceneMinutes(m) => {
                let m = m.min(120);
                self.scene_minutes = m;
                self.settings.set_int("claude/sceneMinutes", m as i64);
                if m == 0 {
                    self.stop_timer(TimerKind::Scene);
                } else if self.mode == DisplayMode::Claude {
                    self.start_timer(TimerKind::Scene, m as u64 * 60_000);
                }
            }
            Command::ClearSessions => {
                if self.claude.clear() {
                    self.sessions_changed();
                }
            }
            Command::SetPort(p) => {
                if p >= 1024 && p != self.port {
                    self.port = p;
                    self.settings.set_int("claude/port", p as i64);
                    self.listening = false;
                    self.restart_http(p);
                    self.hooks_installed = claude::hooks_installed(p);
                }
            }
            Command::InstallHooks => {
                let r = claude::install_hooks(self.port);
                match &r {
                    Ok(_) => self.add_log(tr!("log.hooks_installed")),
                    Err(e) => self.add_log(e.clone()),
                }
                self.hooks_message = Some(match r {
                    Ok(m) => (true, m),
                    Err(e) => (false, e),
                });
                self.hooks_installed = claude::hooks_installed(self.port);
            }
            Command::UninstallHooks => {
                let r = claude::uninstall_hooks();
                if r.is_ok() {
                    self.add_log(tr!("log.hooks_removed"));
                }
                self.hooks_message = Some(match r {
                    Ok(m) => (true, m),
                    Err(e) => (false, e),
                });
                self.hooks_installed = claude::hooks_installed(self.port);
            }
        }
    }

    fn request_missing_thumbs_quiet(&mut self) {
        let missing = self.gallery.missing_thumbs();
        if !missing.is_empty() {
            self.request_missing_thumbs();
        }
    }

    // ------------------------------------------------------------------ HTTP (§14.1)

    fn device_status_json(&self) -> Value {
        json!({
            "connected": self.conn == Conn::Connected,
            "info": Value::Object(self.device_info.clone().into_iter().collect()),
            "frames": self.frames_log.iter().cloned().collect::<Vec<_>>(),
            "mode": mode_id(self.mode),
            "live": self.active_live.unwrap_or(""),
            "liveFramesSent": self.live_frames_sent,
            "lastTransfer": self.last_transfer,
        })
    }

    fn route(&mut self, req: http::Request) -> Response {
        let (m, p) = (req.method.as_str(), req.path.as_str());
        let body_json = || serde_json::from_slice::<Value>(&req.body).unwrap_or(Value::Null);
        if m == "GET" && p.starts_with("/frame/") {
            let id = &p[7..];
            let frame = if id == "device" { self.last_sent_frame.clone() } else { self.modes.get(id).and_then(|s| s.frame.clone()) };
            return match frame {
                Some(f) => Response::png(f.png()),
                None => Response::error(404, "no frame"),
            };
        }
        match (m, p) {
            ("POST", "/hook") => {
                let v = body_json();
                if !v.is_object() {
                    return Response::error(400, "expected hook JSON");
                }
                if self.claude.handle_hook(&v, Local::now()) {
                    self.sessions_changed();
                }
                Response::ok()
            }
            ("POST", _) if p.starts_with("/state/") => match ClaudeState::parse(&p[7..]) {
                Some(s) => {
                    self.claude.force_state(s, Local::now());
                    self.sessions_changed();
                    Response::ok()
                }
                None => Response::error(400, "unknown state"),
            },
            ("POST", "/show") => {
                let v = body_json();
                let Some(path) = v.get("path").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) else {
                    return Response::error(400, "path required");
                };
                let fit = Fit::parse(v.get("fit").and_then(|x| x.as_str()).unwrap_or("crop"));
                self.open_image(PathBuf::from(path), AfterLoad::Send(Some(fit)));
                Response::ok()
            }
            ("POST", "/raw") => {
                let text = String::from_utf8_lossy(&req.body).to_string();
                match protocol::parse_hex(&text) {
                    Some(_) => {
                        self.command(Command::RawHex(text));
                        Response::ok()
                    }
                    None => Response::error(400, "hex body expected"),
                }
            }
            ("GET", "/device") => Response::pretty(&self.device_status_json()),
            ("POST", _) if p.starts_with("/live/") => {
                self.show_live(&p[6..]);
                Response::ok()
            }
            ("POST", "/notify") => {
                let v = body_json();
                let s = |k: &str, d: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or(d).to_string();
                let (app, summary, body, icon) = (s("app", "minitoo"), s("summary", ""), s("body", ""), s("icon", ""));
                self.add_log(tr!("log.notification", app = app, text = summary));
                let card = notify_card::render(&app, &summary, &body, &icon, &crate::i18n::time_hm(&Local::now()));
                self.show_overlay(card, self.notify_duration as u64 * 1000);
                Response::ok()
            }
            ("POST", _) if p.starts_with("/mode/") => {
                match &p[6..] {
                    "claude" => self.set_claude_mode(true),
                    "idle" => {
                        self.set_streaming(false);
                        self.set_mode(DisplayMode::Idle);
                    }
                    _ => {}
                }
                Response::ok()
            }
            ("POST", "/activate") => {
                self.show_serial += 1;
                Response::ok()
            }
            ("GET", "/status") => Response::json(&self.claude.status_json()),
            _ => Response::error(404, "not found"),
        }
    }

    // ------------------------------------------------------------------ snapshot

    fn tray_state_now(&self) -> tray::TrayState {
        let dev = match self.conn {
            Conn::Connected => tr!("app.tray.connected"),
            Conn::Connecting => tr!("app.tray.connecting"),
            Conn::Disconnected => tr!("app.tray.disconnected"),
        };
        let mode_name = match self.mode {
            DisplayMode::Idle => tr!("app.mode.idle"),
            DisplayMode::Image => tr!("app.mode.image"),
            DisplayMode::Screen => tr!("app.mode.screen"),
            DisplayMode::Claude => tr!("app.mode.claude"),
            DisplayMode::Live => tr!("app.mode.live"),
        };
        let state = self.claude.state();
        let mut screen = crate::color::Color::hex(0xd97757);
        if self.mode == DisplayMode::Claude {
            screen = match state {
                ClaudeState::Alerting => crate::color::Color::hex(0xe04030),
                ClaudeState::Working => crate::color::Color::hex(0xd97757),
                ClaudeState::Chilling => crate::color::Color::hex(0x5070b0),
            };
        }
        if self.conn != Conn::Connected {
            screen = crate::color::Color::hex(0x808088);
        }
        tray::TrayState {
            screen,
            tooltip: tr!("app.tray.tooltip", device = dev, mode = mode_name, state = state.id()),
            claude_mode: self.mode == DisplayMode::Claude,
            streaming: self.screen.streaming,
        }
    }

    fn reported(&self) -> Vec<(String, String)> {
        let info = &self.device_info;
        let conf = info.get("json:Sys/DevUpdateConf").cloned().unwrap_or(Value::Null);
        let dash = || "—".to_string();
        let volume = info.get("volume").and_then(|v| v.as_i64()).map(|v| format!("{v} / 15")).unwrap_or_else(dash);
        let brightness = info
            .get("json:Channel/SetBrightness")
            .and_then(|o| o.get("Brightness"))
            .and_then(|v| v.as_i64())
            .map(|v| format!("{v}%"))
            .unwrap_or_else(dash);
        let source = match info.get("workMode").and_then(|v| v.as_i64()) {
            Some(0) => "Bluetooth".into(),
            Some(1) => "FM".into(),
            Some(2) => "Line-in".into(),
            Some(3) => "SD".into(),
            Some(n) => tr!("app.reported.source_mode", n = n),
            None => dash(),
        };
        let sd = match info.get("sdCard").and_then(|v| v.as_i64()) {
            Some(0) => tr!("app.reported.sd_none").into(),
            Some(_) => tr!("app.reported.sd_present").into(),
            None => dash(),
        };
        let num = |k: &str| conf.get(k).and_then(|v| v.as_i64().or_else(|| v.as_bool().map(|b| b as i64)));
        let off = match num("AutoPowerOff") {
            Some(0) => tr!("app.reported.off").into(),
            Some(n) => tr!("app.reported.minutes", n = n),
            None => dash(),
        };
        let t24 = match num("Time24Flag") {
            Some(0) => tr!("app.reported.hours12").into(),
            Some(_) => tr!("app.reported.hours24").into(),
            None => dash(),
        };
        let auto = match num("BluetoothAutoConnect") {
            Some(0) => tr!("app.reported.no").into(),
            Some(_) => tr!("app.reported.yes").into(),
            None => dash(),
        };
        let sound = num("NotificationSound").map(|n| tr!("app.reported.sound_number", n = n)).unwrap_or_else(dash);
        vec![
            (tr!("app.reported.volume").into(), volume),
            (tr!("app.reported.brightness").into(), brightness),
            (tr!("app.reported.source").into(), source),
            (tr!("app.reported.sd_card").into(), sd),
            (tr!("app.reported.auto_off").into(), off),
            (tr!("app.reported.time_format").into(), t24),
            (tr!("app.reported.auto_connect").into(), auto),
            (tr!("app.reported.notify_sound").into(), sound),
        ]
    }

    fn publish(&mut self) {
        if self.gallery_changed {
            self.gallery_changed = false;
            self.gallery_view = Arc::new(self.gallery.view_items());
            self.folder_view = Arc::new(if self.gallery.filter == GalleryFilter::Folder { self.gallery.view_folder() } else { Vec::new() });
        }
        let on_device = self.device_live();
        let modes: Vec<ModeInfo> = self
            .modes
            .slots
            .iter()
            .map(|s| ModeInfo {
                id: s.mode.id(),
                title: s.mode.title(),
                subtitle: s.mode.subtitle(),
                icon: s.mode.icon(),
                status: s.status.clone(),
                frame: s.frame.clone(),
                revision: s.revision,
                in_rotation: self.rotation.contains(s.mode.id()),
                on_device: on_device == Some(s.mode.id()) && self.mode == DisplayMode::Live,
                view: s.mode.view(),
            })
            .collect();
        let next_title = self.modes.get(&self.rotation.next_id()).map(|s| s.mode.title().to_string()).unwrap_or_default();
        let mode_text = match self.mode {
            DisplayMode::Live => self.active_live.and_then(|id| self.modes.get(id)).map(|s| s.mode.title()).unwrap_or(tr!("app.mode.live")),
            DisplayMode::Idle => tr!("app.on_screen.idle"),
            DisplayMode::Image => tr!("app.mode.image"),
            DisplayMode::Screen => tr!("app.mode.screen"),
            DisplayMode::Claude => tr!("app.mode.claude"),
        };
        // the UI prefixes «На экране: » and appends the Claude alert itself
        let on_screen = mode_text.to_string();
        let scenes: Vec<SceneSet> = ClaudeState::ALL
            .iter()
            .map(|&st| {
                let (frames, speed) = self.face(st);
                SceneSet {
                    state: st,
                    current: self.faces.current.get(&st).cloned().unwrap_or_else(|| "classic".into()),
                    off: self.settings.list(&format!("claude/scenesOff/{}", st.id())),
                    custom: self.custom_face_path(st).map(PathBuf::from),
                    anim: Anim { frames, speed, revision: self.faces.revision },
                }
            })
            .collect();
        let snap = Snapshot {
            debug: self.opts.debug,
            theme: self.theme,
            page: self.page,
            mode: self.mode,
            live_on_device: on_device,
            interrupted: self.interrupted,
            overlay: self.overlay_active,
            away: self.away,
            on_screen,
            last_transfer: self.last_transfer.clone(),
            mirror: self.mirror.clone(),
            device: DeviceView {
                conn: self.conn,
                busy: self.inflight > 0,
                battery: self.battery,
                audio_connected: self.audio_connected,
                screen_on: self.device_info.get("screenOn").and_then(|v| v.as_bool()).unwrap_or(true),
                brightness: self.brightness,
                volume: self.device_info.get("volume").and_then(|v| v.as_i64()).map(|v| v.clamp(0, 15) as u8),
                playing: self.device_info.get("playing").and_then(|v| v.as_bool()),
                reported: self.reported(),
                heartbeat: self.device_info.get("heartbeat").and_then(|v| v.as_str()).map(String::from),
                away_enabled: self.away_enabled,
                away_brightness: self.away_brightness,
                locked: self.away,
                discovering: self.discovering,
                discovered: self.discovered.clone(),
                info_json: self.device_status_json(),
            },
            settings: SettingsView {
                mac: self.mac.clone(),
                channel: self.settings.int_in("device/channel", 1, 1, 30) as u8,
                auto_connect: self.settings.bool("device/autoConnect", true),
                keepalive: self.settings.int_in("device/keepalive", 60, 0, 600) as u32,
                chunk_delay: self.settings.int_in("device/chunkDelay", 2, 0, 60) as u32,
                zstd_level: self.zstd_level(),
                close_to_tray: self.settings.bool("ui/closeToTray", true),
                start_hidden: self.settings.bool("ui/startHidden", false),
                language: self.settings.string("ui/language", "auto"),
                language_used: self.language_used.clone(),
                time_format: self.settings.string("ui/timeFormat", "auto"),
                date_format: self.settings.string("ui/dateFormat", "auto"),
            },
            image: ImageState {
                source: self.image.as_ref().map(|d| d.source.clone()),
                fit: self.fit,
                pixel_art: self.pixel_art,
                crop: self.crop,
                preview: self.preview.clone(),
                source_frames: self.image.as_ref().map(|d| d.anim.frames.len()).unwrap_or(0),
                gallery: self.gallery_view.clone(),
                folder: self.gallery.folder().map(Path::to_path_buf),
                folder_items: self.folder_view.clone(),
                filter: self.gallery.filter,
                error: self.image_error.clone(),
            },
            screen: ScreenState {
                status: self.screen.status,
                has_token: self.screen.restore_token.is_some(),
                source_size: self.screen.source_size,
                live: self.screen.preview.clone(),
                live_counter: self.screen.preview_counter,
                region: self.screen.region,
                fps: self.screen.fps,
                crisp: self.screen.crisp,
                quality: self.screen.quality,
                streaming: self.screen.streaming,
                paused: self.screen.streaming && self.screen.paused,
                actual_fps: self.screen.actual_fps,
                frame_kb: self.screen.last_bytes as f32 / 1024.0,
                error: self.screen.error.clone(),
                stale_portals: self.screen.stale.clone(),
            },
            modes,
            rotation: RotationState {
                running: self.rotation.running(),
                interval: self.rotation.interval(),
                checked: self.rotation.count(),
                paused: self.rotation.paused(),
                progress: self.rotation.progress(),
                seconds_left: self.rotation.remaining(),
                next_title,
            },
            notify: NotifyState {
                enabled: self.notify_enabled,
                duration: self.notify_duration,
                ignore: self.notify_ignore.clone(),
                error: self.notify_error.clone(),
            },
            claude: ClaudeView {
                mode_on: self.mode == DisplayMode::Claude,
                state: self.claude.state(),
                sessions: self.claude.sessions(),
                interrupt: self.claude_interrupt,
                idle_alerts: self.claude.idle_alerts,
                alert_caption: self.alert_caption,
                scene_minutes: self.scene_minutes,
                scenes,
                port: self.port,
                listening: self.listening,
                port_busy: self.port_busy,
                hooks_installed: self.hooks_installed,
                hooks_path: claude::claude_settings_path().to_string_lossy().into_owned(),
                hooks_message: self.hooks_message.clone(),
                snippet: claude::hooks_snippet(self.port),
            },
            log: self.log_arc.clone(),
            http_listening: self.listening,
            show_window_serial: self.show_serial,
            quit: self.quit,
        };
        *self.core.0.snapshot.write() = Arc::new(snap);
        self.core.request_repaint();
        let ts = self.tray_state_now();
        if self.tray_state.as_ref() != Some(&ts) {
            if let Some(t) = &self.tray {
                t.update(ts.clone());
            }
            self.tray_state = Some(ts);
        }
    }
}

pub fn mode_id(m: DisplayMode) -> &'static str {
    match m {
        DisplayMode::Idle => "idle",
        DisplayMode::Image => "image",
        DisplayMode::Screen => "screen",
        DisplayMode::Live => "live",
        DisplayMode::Claude => "claude",
    }
}
