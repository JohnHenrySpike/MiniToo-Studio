//! Contract between the core (controller loop) and front-ends (window, tray, HTTP, CLI).
//!
//! The core publishes an immutable [`Snapshot`] after every change; front-ends read the latest
//! one with [`CoreHandle::snapshot`] and send [`Command`]s. Heavy data (frames, images) is
//! behind `Arc`, so cloning a snapshot is cheap.

use crate::claude::{ClaudeState, Session};
use crate::frame::Frame;
use crate::live::{ModeCommand, ModeView};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

/// Normalised rectangle in source coordinates, 0..1.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct NRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl NRect {
    pub const FULL: NRect = NRect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };

    /// The largest centred rectangle with the 5:4 aspect ratio for a `sw × sh` source.
    pub fn center_5x4(sw: f64, sh: f64) -> NRect {
        if sw <= 0.0 || sh <= 0.0 {
            return NRect::FULL;
        }
        let target = 5.0 / 4.0;
        if sw / sh > target {
            let w = sh * target / sw;
            NRect { x: (1.0 - w) / 2.0, y: 0.0, w, h: 1.0 }
        } else {
            let h = sw / target / sh;
            NRect { x: 0.0, y: (1.0 - h) / 2.0, w: 1.0, h }
        }
    }

    pub fn clamped(self) -> NRect {
        let w = self.w.clamp(0.001, 1.0);
        let h = self.h.clamp(0.001, 1.0);
        NRect { x: self.x.clamp(0.0, 1.0 - w), y: self.y.clamp(0.0, 1.0 - h), w, h }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Conn {
    #[default]
    Disconnected = 0,
    Connecting = 1,
    Connected = 2,
}

/// `mode` of §4.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DisplayMode {
    #[default]
    Idle,
    Image,
    Screen,
    Live,
    Claude,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Fit {
    #[default]
    Crop = 0,
    Fit = 1,
    Stretch = 2,
}

impl Fit {
    pub fn from_i64(v: i64) -> Fit {
        match v {
            1 => Fit::Fit,
            2 => Fit::Stretch,
            _ => Fit::Crop,
        }
    }
    pub fn parse(s: &str) -> Fit {
        match s {
            "fit" => Fit::Fit,
            "stretch" => Fit::Stretch,
            _ => Fit::Crop,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    Beige,
    Dark,
}

/// Frames as the device plays them (mirror, previews).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Anim {
    pub frames: Arc<Vec<Frame>>,
    /// ms per frame
    pub speed: u32,
    /// changes whenever the content changes
    pub revision: u64,
}

// ---------------------------------------------------------------------- image page

/// The open source image (decoded), for the editor canvas.
#[derive(Clone, Debug)]
pub struct SourceImage {
    pub path: PathBuf,
    pub name: String,
    /// original size
    pub width: u32,
    pub height: u32,
    /// display frames: the original frames, downscaled to at most ~1024 px for the editor
    pub frames: Arc<Vec<Arc<image::RgbaImage>>>,
    pub delays: Arc<Vec<u32>>,
    /// gallery id (SHA-1 prefix)
    pub id: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GalleryItemView {
    pub id: String,
    pub name: String,
    pub file: PathBuf,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub sent: u32,
    pub favorite: bool,
    /// 160×128 thumbnail "as on the device"; `None` while it is being rendered
    pub thumb: Option<Frame>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FolderItemView {
    pub path: PathBuf,
    pub name: String,
    pub thumb: Option<Frame>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GalleryFilter {
    #[default]
    All,
    Favorites,
    Folder,
}

#[derive(Clone, Debug, Default)]
pub struct ImageState {
    pub source: Option<SourceImage>,
    pub fit: Fit,
    pub pixel_art: bool,
    pub crop: NRect,
    /// "Так будет на колонке"
    pub preview: Anim,
    /// frame count of the source (for the >92 warning)
    pub source_frames: usize,
    pub gallery: Arc<Vec<GalleryItemView>>,
    pub folder: Option<PathBuf>,
    pub folder_items: Arc<Vec<FolderItemView>>,
    pub filter: GalleryFilter,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------- screen page

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CaptureStatus {
    #[default]
    Idle,
    /// waiting for the portal dialog
    Selecting,
    Capturing,
}

#[derive(Clone, Debug, Default)]
pub struct ScreenState {
    pub status: CaptureStatus,
    pub has_token: bool,
    pub source_size: Option<(u32, u32)>,
    /// latest captured frame (downscaled to ≤ 960 px wide) and a counter
    pub live: Option<Arc<image::RgbaImage>>,
    pub live_counter: u64,
    pub region: NRect,
    pub fps: u32,
    pub crisp: bool,
    /// 0 Максимум, 1 Баланс, 2 Скорость
    pub quality: i64,
    pub streaming: bool,
    pub paused: bool,
    pub actual_fps: f32,
    pub frame_kb: f32,
    pub error: Option<String>,
    /// stale portal units (`systemctl --user` names) that should be restarted
    pub stale_portals: Vec<String>,
}

// ---------------------------------------------------------------------- live modes

#[derive(Clone, Debug, Default)]
pub struct ModeInfo {
    pub id: &'static str,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub icon: &'static str,
    pub status: String,
    pub frame: Option<Frame>,
    pub revision: u64,
    pub in_rotation: bool,
    pub on_device: bool,
    pub view: ModeView,
}

#[derive(Clone, Debug, Default)]
pub struct RotationState {
    pub running: bool,
    pub interval: u32,
    pub checked: usize,
    pub paused: bool,
    /// 0..1 towards the next switch
    pub progress: f32,
    pub seconds_left: u32,
    pub next_title: String,
}

#[derive(Clone, Debug, Default)]
pub struct NotifyState {
    pub enabled: bool,
    pub duration: u32,
    pub ignore: Vec<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------- claude

#[derive(Clone, Debug, Default)]
pub struct SceneSet {
    pub state: ClaudeState,
    /// current scene id (built-in variant)
    pub current: String,
    /// variants switched off
    pub off: Vec<String>,
    /// custom GIF path replacing all scenes
    pub custom: Option<PathBuf>,
    /// what this state shows now (custom GIF or current scene), with caption if alerting
    pub anim: Anim,
}

#[derive(Clone, Debug, Default)]
pub struct ClaudeView {
    pub mode_on: bool,
    pub state: ClaudeState,
    pub sessions: Vec<Session>,
    pub interrupt: bool,
    pub idle_alerts: bool,
    pub alert_caption: bool,
    pub scene_minutes: u32,
    pub scenes: Vec<SceneSet>,
    pub port: u16,
    pub listening: bool,
    pub port_busy: bool,
    pub hooks_installed: bool,
    pub hooks_path: String,
    pub hooks_message: Option<(bool, String)>,
    pub snippet: String,
}

// ---------------------------------------------------------------------- device

#[derive(Clone, Debug, Default)]
pub struct DeviceView {
    pub conn: Conn,
    pub busy: bool,
    pub battery: Option<u8>,
    pub audio_connected: bool,
    pub screen_on: bool,
    pub brightness: u8,
    pub volume: Option<u8>,
    pub playing: Option<bool>,
    /// «Что сообщает колонка»: (label, value)
    pub reported: Vec<(String, String)>,
    pub heartbeat: Option<String>,
    pub away_enabled: bool,
    pub away_brightness: u8,
    pub locked: bool,
    pub discovering: bool,
    pub discovered: Vec<(String, String)>,
    pub info_json: serde_json::Value,
}

#[derive(Clone, Debug, Default)]
pub struct SettingsView {
    pub mac: String,
    pub channel: u8,
    pub auto_connect: bool,
    pub keepalive: u32,
    pub chunk_delay: u32,
    pub zstd_level: i32,
    pub close_to_tray: bool,
    pub start_hidden: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub debug: bool,
    pub theme: Theme,
    pub page: usize,
    pub mode: DisplayMode,
    /// the live mode on the device (incl. the lock-screen clock)
    pub live_on_device: Option<&'static str>,
    pub interrupted: bool,
    pub overlay: bool,
    pub away: bool,
    /// «На экране: …»
    pub on_screen: String,
    /// «Часы и погода: 4.2 КБ, 310 мс»
    pub last_transfer: String,
    pub mirror: Anim,
    pub device: DeviceView,
    pub settings: SettingsView,
    pub image: ImageState,
    pub screen: ScreenState,
    pub modes: Vec<ModeInfo>,
    pub rotation: RotationState,
    pub notify: NotifyState,
    pub claude: ClaudeView,
    pub log: Arc<Vec<String>>,
    pub http_listening: bool,
    /// set when the window should be shown (activate from another instance / tray)
    pub show_window_serial: u64,
    pub quit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Cosmonaut,
    Gallery,
    NoiseMeter,
    Tetris,
    Game2,
    Game3,
    ExitGame,
}

#[derive(Clone, Debug)]
pub enum Command {
    // ---- window / app
    SetPage(usize),
    SetTheme(Theme),
    ShowWindow,
    Quit,
    /// the Modes page is visible: keep every mode running for the live thumbnails
    PreviewModes(bool),
    /// the Claude page is visible
    PreviewClaude(bool),

    // ---- device
    Connect(bool),
    SetBrightness(u8),
    SetVolume(u8),
    PlayPause,
    PrevTrack,
    NextTrack,
    ScreenOnOff(bool),
    SyncTime,
    Builtin(Builtin),
    Scoreboard { red: u16, blue: u16 },
    /// text card from one of the 12 apps
    DeviceNotifyCard { app: String, text: String },
    /// firmware popup `0x50` (icon only)
    DeviceNotifyIcon { app: String },
    RawHex(String),
    ConnectAudio,
    Discover,
    SetAwayEnabled(bool),
    SetAwayBrightness(u8),

    // ---- settings
    SetMac(String),
    SetChannel(u8),
    SetAutoConnect(bool),
    SetKeepalive(u32),
    SetChunkDelay(u32),
    SetZstdLevel(i32),
    SetCloseToTray(bool),
    SetStartHidden(bool),

    // ---- image + gallery
    OpenFiles(Vec<PathBuf>),
    /// drop: add all (folders: top-level files); a single file is opened as well
    AddPaths(Vec<PathBuf>),
    SetFit(Fit),
    SetPixelArt(bool),
    SetCrop(NRect),
    ResetCrop,
    SendImage,
    GalleryOpen(String),
    GallerySend(String),
    GalleryFavorite(String, bool),
    GalleryRemove(String),
    GalleryFilter(GalleryFilter),
    SetFolder(Option<PathBuf>),
    RescanFolder,
    FolderOpen(PathBuf),
    FolderSend(PathBuf),

    // ---- screen
    /// open the portal dialog (forget the remembered source)
    SelectSource,
    /// start with the remembered source
    StartCapture,
    StopCapture,
    SetRegion(NRect),
    StartStream,
    StopStream,
    SetFps(u32),
    SetCrisp(bool),
    SetQuality(i64),
    RestartPortals,

    // ---- live modes
    ShowLive(String),
    /// stop updating the device; the picture stays
    StopLive,
    Mode(String, ModeCommand),
    SetRotationMember(String, bool),
    StartRotation,
    RotationNext,
    SetRotationInterval(u32),
    SetNotifyEnabled(bool),
    SetNotifyDuration(u32),
    SetNotifyIgnore(Vec<String>),
    TestNotification,

    // ---- claude
    SetClaudeMode(bool),
    SetInterrupt(bool),
    SetIdleAlerts(bool),
    SetAlertCaption(bool),
    /// 10 s fake session + overlay on the device
    TestState(ClaudeState),
    /// overlay 10 s with the current scene of this state
    ShowStateOnDevice(ClaudeState),
    /// make the scene current and show it (overlay 10 s)
    PickScene(ClaudeState, String),
    SetSceneEnabled(ClaudeState, String, bool),
    SetCustomFace(ClaudeState, Option<PathBuf>),
    SetSceneMinutes(u32),
    ClearSessions,
    SetPort(u16),
    InstallHooks,
    UninstallHooks,

    // ---- added by the UI
    /// stop the rotation; the current mode stays on the device
    StopRotation,
    /// clear the log shown in the right panel (`--debug`)
    ClearLog,
}

/// Shared between the core and front-ends.
pub struct CoreShared {
    pub snapshot: RwLock<Arc<Snapshot>>,
    pub tx: tokio::sync::mpsc::UnboundedSender<Command>,
    pub repaint: RwLock<Option<Box<dyn Fn() + Send + Sync>>>,
}

#[derive(Clone)]
pub struct CoreHandle(pub Arc<CoreShared>);

impl CoreHandle {
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.0.snapshot.read().clone()
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.0.tx.send(cmd);
    }

    /// Called by the core after publishing a new snapshot.
    pub fn set_repaint(&self, f: Box<dyn Fn() + Send + Sync>) {
        *self.0.repaint.write() = Some(f);
    }

    pub fn request_repaint(&self) {
        if let Some(f) = self.0.repaint.read().as_ref() {
            f();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_5x4() {
        let r = NRect::center_5x4(1920.0, 1080.0);
        assert!((r.h - 1.0).abs() < 1e-9);
        assert!((r.w * 1920.0 / (r.h * 1080.0) - 1.25).abs() < 1e-9);
        let r = NRect::center_5x4(100.0, 200.0);
        assert!((r.w - 1.0).abs() < 1e-9 && (r.y - (1.0 - r.h) / 2.0).abs() < 1e-9);
    }
}
