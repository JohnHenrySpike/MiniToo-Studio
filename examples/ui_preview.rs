//! The window with a mock core: a rich fake snapshot and a small loop applying commands to it.
//!
//! ```text
//! cargo run --example ui_preview [-- --debug] [--hidden] [--screenshot DIR]
//! ```

use minitoo::api::*;
use minitoo::canvas::{Align, Canvas, LineCap, r};
use minitoo::claude::{ClaudeState, Session};
use minitoo::color::Color;
use minitoo::fonts::FontSpec;
use minitoo::frame::Frame;
use minitoo::live::{City, ClockView, GithubView, ModeCommand, ModeView, NowPlayingView, PomodoroView, RepoView, RunState, VisualizerView};
use minitoo::ui::{UiExit, UiOptions};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const PINK: Color = Color::hex(0xff3cac);
const CYAN: Color = Color::hex(0x38e8ff);

fn clock_frame(colon: bool) -> Frame {
    let mut c = Canvas::device();
    c.fill(Color::hex(0x06040e));
    for i in 0..5 {
        let y = 96.0 + i as f32 * 8.0;
        c.line(0.0, y, 160.0, y, 1.0, Color::hex(0x280e34));
    }
    for i in -6..=6 {
        c.line(80.0, 96.0, 80.0 + i as f32 * 30.0, 128.0, 1.0, Color::hex(0x280e34));
    }
    c.text(r(0.0, 4.0, 160.0, 18.0), Align::CENTER, "2026-10-09 Fri", FontSpec::bold(12.0), CYAN);
    c.stroke_round_rect(12.0, 30.0, 136.0, 50.0, 7.0, 2.0, PINK);
    let t = if colon { "20:48" } else { "20 48" };
    c.text(r(12.0, 30.0, 136.0, 50.0), Align::CENTER, t, FontSpec::bold(38.0), PINK);
    let pts: Vec<(f32, f32)> = (0..=40).map(|i| {
        let x = 8.0 + i as f32 * 3.6;
        let y = if i % 10 == 5 { 92.0 } else if i % 10 == 6 { 114.0 } else { 104.0 };
        (x, y)
    }).collect();
    c.polyline(&pts, 2.0, CYAN);
    c.to_frame()
}

fn sysmon_frame(cpu: f32) -> Frame {
    let mut c = Canvas::device();
    c.fill(Color::hex(0x0a0e16));
    c.text_tl(8.0, 2.0, "Система", FontSpec::bold(12.0), Color::WHITE);
    c.text(r(100.0, 2.0, 52.0, 14.0), Align::RIGHT, "20:48", FontSpec::sans(10.0), Color::hex(0x96a0b4));
    let rows = [("CPU", cpu, Color::hex(0x5ac878)), ("GPU", 0.32, Color::hex(0xf0aa3c)), ("RAM", 0.41, Color::hex(0x6e96f0)), ("VRAM", 0.18, Color::hex(0xb478e6))];
    for (i, (name, v, col)) in rows.iter().enumerate() {
        let y = 22.0 + i as f32 * 22.0;
        c.text_tl(8.0, y, name, FontSpec::bold(9.0), Color::WHITE);
        c.text(r(80.0, y, 72.0, 12.0), Align::RIGHT, &format!("{}%", (v * 100.0) as i32), FontSpec::sans(9.0), Color::hex(0xc8cddc));
        c.bar(8.0, y + 13.0, 144.0, 5.0, *v, *col, None);
    }
    let pts: Vec<(f32, f32)> = (0..48).map(|i| (8.0 + i as f32 * 3.0, 124.0 - 10.0 * ((i as f32 * 0.7 + cpu * 9.0).sin() * 0.5 + 0.5))).collect();
    c.polyline(&pts, 1.2, Color::hex(0x5ac878));
    c.to_frame()
}

fn nowplaying_frame() -> Frame {
    let mut c = Canvas::device();
    c.fill(Color::hex(0x0e0c16));
    c.vgradient(8.0, 8.0, 62.0, 62.0, Color::hex(0x3a2a80), Color::hex(0xb05a9a));
    c.fill_circle(39.0, 42.0, 18.0, Color::hex(0xf6b25e));
    c.text_wrapped(r(76.0, 8.0, 80.0, 34.0), Align::TOP_LEFT, "Midnight City", FontSpec::bold(12.0), Color::WHITE, 2, 0.0);
    c.text_tl(76.0, 42.0, "M83", FontSpec::sans(10.0), Color::hex(0xcdcddc));
    c.text_tl(76.0, 56.0, "Spotify", FontSpec::sans(9.0), Color::hex(0x8cdc96));
    c.bar(8.0, 84.0, 144.0, 4.0, 0.53, Color::ACCENT, Some(Color::hex(0x46465a)));
    c.text_tl(8.0, 90.0, "2:14", FontSpec::sans(9.0), Color::hex(0xc8c8d2));
    c.text(r(100.0, 90.0, 52.0, 12.0), Align::RIGHT, "4:03", FontSpec::sans(9.0), Color::hex(0xc8c8d2));
    c.fill_rect(74.0, 106.0, 4.0, 12.0, Color::WHITE);
    c.fill_rect(82.0, 106.0, 4.0, 12.0, Color::WHITE);
    c.fill_polygon(&[(42.0, 112.0), (50.0, 106.0), (50.0, 118.0)], Color::WHITE);
    c.fill_polygon(&[(118.0, 112.0), (110.0, 106.0), (110.0, 118.0)], Color::WHITE);
    c.to_frame()
}

fn pomodoro_frame() -> Frame {
    let mut c = Canvas::device();
    c.fill(Color::hex(0x160c0c));
    c.arc(80.0, 56.0, 42.5, 0.0, 360.0, 7.0, Color::ACCENT.darker(3.2), LineCap::Round);
    c.arc(80.0, 56.0, 42.5, 90.0, -250.0, 7.0, Color::ACCENT, LineCap::Round);
    c.text(r(0.0, 38.0, 160.0, 28.0), Align::CENTER, "25:00", FontSpec::bold(24.0), Color::WHITE);
    c.text(r(0.0, 64.0, 160.0, 14.0), Align::CENTER, "фокус · пауза", FontSpec::sans(10.0), Color::ACCENT.lighter(1.4));
    for i in 0..4 {
        c.fill_circle(59.0 + 14.0 * i as f32, 116.0, 4.0, if i == 0 { Color::ACCENT } else { Color::ACCENT.darker(3.0) });
    }
    c.to_frame()
}

fn stats_frame() -> Frame {
    let mut c = Canvas::device();
    c.fill(Color::hex(0x120f16));
    c.fill_rect(0.0, 0.0, 160.0, 20.0, Color::ACCENT);
    c.text_tl(6.0, 3.0, "Claude Code", FontSpec::bold(12.0), Color::hex(0x1e120e));
    c.text(r(120.0, 3.0, 34.0, 14.0), Align::RIGHT, "2", FontSpec::bold(12.0), Color::hex(0x1e120e));
    let rows = [("divoom", "работает", Color::ACCENT), ("website", "ждёт вас", Color::hex(0xe64030))];
    for (i, (p, s, col)) in rows.iter().enumerate() {
        let y = 26.0 + i as f32 * 20.0;
        c.fill_circle(10.0, y + 7.0, 4.0, *col);
        c.text_tl(20.0, y, p, FontSpec::bold(11.0), Color::WHITE);
        c.text(r(90.0, y, 64.0, 14.0), Align::RIGHT, s, FontSpec::sans(10.0), *col);
    }
    c.line(6.0, 90.0, 154.0, 90.0, 1.0, Color::hex(0x373241));
    c.text_tl(6.0, 96.0, "сегодня", FontSpec::sans(9.0), Color::hex(0x9691a5));
    c.text(r(60.0, 94.0, 94.0, 16.0), Align::RIGHT, "62.3M ток.", FontSpec::bold(13.0), Color::WHITE);
    c.text_tl(6.0, 114.0, "ответов 412", FontSpec::sans(9.0), Color::hex(0x9691a5));
    c.to_frame()
}

fn github_frame() -> Frame {
    let mut c = Canvas::device();
    c.fill(Color::hex(0x0d1117));
    c.text_tl(6.0, 5.0, "GitHub Actions", FontSpec::bold(11.0), Color::WHITE);
    let rows = [("✓", "cli", "passed", Color::hex(0x3cbe64)), ("✗", "neovim", "failed", Color::hex(0xe6463c)), ("●", "rust", "running", Color::hex(0xe6b432))];
    for (i, (icon, name, s, col)) in rows.iter().enumerate() {
        let y = 24.0 + i as f32 * 25.0;
        c.fill_round_rect(6.0, y, 148.0, 22.0, 4.0, Color::hex(0x181e28));
        c.text_tl(11.0, y + 3.0, icon, FontSpec::bold(12.0), *col);
        c.text_tl(28.0, y + 5.0, name, FontSpec::bold(10.0), Color::WHITE);
        c.text(r(90.0, y, 60.0, 22.0), Align::RIGHT, s, FontSpec::sans(9.0), *col);
    }
    c.to_frame()
}

fn visualizer_frame(t: f32) -> Frame {
    let mut c = Canvas::device();
    c.fill(Color::BLACK);
    for b in 0..20 {
        let level = ((t * 3.0 + b as f32 * 0.6).sin() * 0.5 + 0.5) * (1.0 - b as f32 / 30.0);
        let segs = (level * 21.0) as i32;
        for s in 0..segs {
            let k = s as f32 / 21.0;
            let col = Color::rgb(80, 200, 255).mix(Color::rgb(255, 80, 155), k);
            c.fill_rect(6.0 + b as f32 * 7.4, 122.0 - s as f32 * 5.0 - 3.0, 5.0, 3.0, col);
        }
    }
    c.to_frame()
}

fn sunset(i: usize) -> image::RgbaImage {
    let mut c = Canvas::new(320, 256);
    c.vgradient(0.0, 0.0, 320.0, 256.0, Color::hex(0x23104a), Color::hex(0xc0507a));
    let y = 150.0 + (i as f32 * 0.8).sin() * 18.0;
    c.fill_circle(160.0, y, 60.0, Color::hex(0xf6b25e));
    c.fill_rect(0.0, 190.0, 320.0, 66.0, Color::hex(0x120d24));
    c.text(r(0.0, 20.0, 320.0, 40.0), Align::CENTER, "SUNSET", FontSpec::bold(34.0), Color::WHITE);
    c.to_image()
}

fn thumb(hue: f32, label: &str) -> Frame {
    let mut c = Canvas::device();
    let a = Color::from_hsv(hue, 0.6, 0.35);
    let b = Color::from_hsv((hue + 0.15) % 1.0, 0.7, 0.9);
    c.vgradient(0.0, 0.0, 160.0, 128.0, a, b);
    c.fill_circle(120.0, 40.0, 18.0, Color::hex(0xfff1c0));
    c.text(r(0.0, 70.0, 160.0, 40.0), Align::CENTER, label, FontSpec::bold(22.0), Color::WHITE);
    c.to_frame()
}

fn scene_anim(state: ClaudeState, variant: &str, revision: u64) -> Anim {
    let scene = minitoo::faces::generate(state, variant);
    let speed = if scene.delays.is_empty() { 200 } else { (scene.delays.iter().sum::<u32>() / scene.delays.len() as u32).max(20) };
    Anim { frames: Arc::new(scene.frames), speed, revision }
}

fn clock_view(style: i64, city: Option<City>, results: Vec<City>) -> ModeView {
    ModeView::Clock(ClockView { style, city, results, searching: false, search_error: None })
}

fn build() -> Snapshot {
    let now = chrono::Local::now();
    let src_frames: Vec<Arc<image::RgbaImage>> = (0..6).map(|i| Arc::new(sunset(i))).collect();
    let preview: Vec<Frame> = src_frames.iter().map(|f| Frame::from_image(f)).collect();
    let names = ["sunset.gif", "cat.png", "pixel-forest.png", "nyan.gif", "logo.webp", "sky.jpg", "mario.png", "wave.gif"];
    let gallery: Vec<GalleryItemView> = names
        .iter()
        .enumerate()
        .map(|(i, n)| GalleryItemView {
            id: format!("{:08x}", 0x1a2b3c00u32 + i as u32),
            name: n.to_string(),
            file: PathBuf::from(format!("/tmp/{n}")),
            width: 320,
            height: 256,
            frames: if n.ends_with(".gif") { 24 } else { 1 },
            sent: i as u32 % 3,
            favorite: i == 1 || i == 3,
            thumb: if i == 7 { None } else { Some(if i == 0 { preview[0].clone() } else { thumb(i as f32 / 8.0, &n[..n.find('.').unwrap()]) }) },
        })
        .collect();
    let mode = |id: &'static str, title: &'static str, subtitle: &'static str, icon: &'static str, status: &str, frame: Frame, view: ModeView, rot: bool| ModeInfo {
        id,
        title,
        subtitle,
        icon,
        status: status.into(),
        frame: Some(frame),
        revision: 1,
        in_rotation: rot,
        on_device: id == "clock",
        view,
    };
    let modes = vec![
        mode("clock", "Часы и погода", "время, дата, Open-Meteo", "clock", "укажите город для погоды", clock_frame(true), clock_view(1, None, vec![]), true),
        mode("sysmon", "Системный монитор", "CPU, GPU, память, температуры", "sysmon", "CPU 12%  ·  RAM 3.9 ГБ", sysmon_frame(0.12), ModeView::None, true),
        mode(
            "nowplaying",
            "Сейчас играет",
            "обложка и трек из любого плеера (MPRIS)",
            "music",
            "Spotify: M83 — Midnight City",
            nowplaying_frame(),
            ModeView::NowPlaying(NowPlayingView { available: true, player: "Spotify".into(), artist: "M83".into(), title: "Midnight City".into(), playing: true }),
            true,
        ),
        mode(
            "pomodoro",
            "Pomodoro",
            "фокус 25 мин, перерыв 5 мин",
            "timer",
            "фокус 25:00 (пауза)",
            pomodoro_frame(),
            ModeView::Pomodoro(PomodoroView { remaining: 1500, cycle: 1, work_min: 25, break_min: 5, long_min: 15, ..Default::default() }),
            false,
        ),
        mode("claudestats", "Статистика Claude", "сессии и токены за сегодня", "sparkle", "сегодня: 62.3M токенов (вход 510K, выход 1.2M, кэш 60.6M), ответов 412, запросов 37", stats_frame(), ModeView::None, false),
        mode(
            "github",
            "GitHub Actions",
            "последние запуски CI по репозиториям",
            "branch",
            "упавших: 1 из 3",
            github_frame(),
            ModeView::Github(GithubView {
                repos: vec![
                    RepoView { name: "cli/cli".into(), state: RunState::Passed, detail: "успешно  ·  CI #8123 (trunk)".into(), url: String::new() },
                    RepoView { name: "neovim/neovim".into(), state: RunState::Failed, detail: "упал  ·  test #4410 (master)".into(), url: String::new() },
                    RepoView { name: "rust-lang/rust".into(), state: RunState::Running, detail: "идёт  ·  CI #99120 (master)".into(), url: String::new() },
                ],
                has_token: false,
                add_error: None,
                interval: 270,
            }),
            false,
        ),
        mode("visualizer", "Визуализатор звука", "спектр того, что играет на компьютере", "wave", "слушаю системный звук", visualizer_frame(0.0), ModeView::Visualizer(VisualizerView { style: 0, error: None }), false),
    ];
    let sessions = vec![
        Session {
            id: "a1b2c3d4e5".into(),
            cwd: "/home/spike/projects/website".into(),
            state: ClaudeState::Alerting,
            last_event: "Notification".into(),
            message: "Claude needs your permission to use Bash".into(),
            updated: now - chrono::Duration::seconds(12),
        },
        Session {
            id: "f6e5d4c3b2".into(),
            cwd: "/home/spike/projects/divoom-studio-rust".into(),
            state: ClaudeState::Working,
            last_event: "PreToolUse".into(),
            message: String::new(),
            updated: now - chrono::Duration::seconds(140),
        },
    ];
    let scenes = ClaudeState::ALL
        .iter()
        .map(|&s| {
            let cur = minitoo::faces::variants(s)[0].to_string();
            SceneSet { state: s, current: cur.clone(), off: if s == ClaudeState::Working { vec!["juggle".into()] } else { vec![] }, custom: None, anim: scene_anim(s, &cur, 1) }
        })
        .collect();
    Snapshot {
        debug: false,
        theme: Theme::Beige,
        page: 0,
        mode: DisplayMode::Live,
        live_on_device: Some("clock"),
        on_screen: "Часы и погода".into(),
        last_transfer: "Часы и погода: 4.2 КБ, 310 мс".into(),
        mirror: Anim { frames: Arc::new(vec![clock_frame(true), clock_frame(false)]), speed: 1000, revision: 1 },
        device: DeviceView {
            conn: Conn::Connected,
            battery: Some(90),
            screen_on: true,
            brightness: 80,
            volume: Some(7),
            playing: Some(false),
            reported: vec![
                ("Громкость".into(), "7 / 15".into()),
                ("Яркость".into(), "80%".into()),
                ("Источник звука".into(), "Bluetooth".into()),
                ("SD-карта".into(), "нет".into()),
                ("Автовыключение".into(), "30 мин".into()),
                ("Формат времени".into(), "24 ч".into()),
                ("Автоподключение".into(), "да".into()),
                ("Звук уведомлений".into(), "№ 2".into()),
            ],
            heartbeat: Some("f7 01 00 5a 22 (20:48:11)".into()),
            away_enabled: true,
            away_brightness: 15,
            ..Default::default()
        },
        settings: SettingsView {
            mac: "B1:21:81:05:E2:65".into(),
            channel: 1,
            auto_connect: true,
            keepalive: 60,
            chunk_delay: 2,
            zstd_level: 19,
            close_to_tray: false,
            start_hidden: false,
        },
        image: ImageState {
            source: Some(SourceImage {
                path: PathBuf::from("/tmp/sunset.gif"),
                name: "sunset.gif".into(),
                width: 320,
                height: 256,
                frames: Arc::new(src_frames),
                delays: Arc::new(vec![160; 6]),
                id: gallery[0].id.clone(),
            }),
            fit: Fit::Crop,
            pixel_art: false,
            crop: NRect { x: 0.1, y: 0.1, w: 0.8, h: 0.8 },
            preview: Anim { frames: Arc::new(preview), speed: 160, revision: 1 },
            source_frames: 6,
            gallery: Arc::new(gallery),
            ..Default::default()
        },
        screen: ScreenState { fps: 5, quality: 1, crisp: false, region: NRect::FULL, ..Default::default() },
        modes,
        rotation: RotationState { running: true, interval: 30, checked: 3, paused: false, progress: 0.3, seconds_left: 21, next_title: "Системный монитор".into() },
        notify: NotifyState { enabled: false, duration: 6, ignore: vec!["Spectacle".into()], error: None },
        claude: ClaudeView {
            mode_on: false,
            state: ClaudeState::Alerting,
            sessions,
            interrupt: true,
            idle_alerts: false,
            alert_caption: true,
            scene_minutes: 5,
            scenes,
            port: 47800,
            listening: true,
            port_busy: false,
            hooks_installed: true,
            hooks_path: "~/.claude/settings.json".into(),
            hooks_message: None,
            snippet: minitoo::claude::hooks_snippet(47800),
        },
        log: Arc::new(vec![
            "20:47:58  подключено".into(),
            "20:48:00  Часы и погода: 4.2 КБ, 310 мс".into(),
            "20:48:05  заряд колонки: 90%".into(),
        ]),
        http_listening: true,
        ..Default::default()
    }
}

fn log(s: &mut Snapshot, line: String) {
    let mut l = (*s.log).clone();
    l.push(format!("{}  {line}", chrono::Local::now().format("%H:%M:%S")));
    s.log = Arc::new(l);
}

fn find_mode<'a>(s: &'a mut Snapshot, id: &str) -> Option<&'a mut ModeInfo> {
    s.modes.iter_mut().find(|m| m.id == id)
}

fn show_live(s: &mut Snapshot, id: &str) {
    let mut title = String::new();
    for m in &mut s.modes {
        m.on_device = m.id == id;
        if m.on_device {
            title = m.title.to_string();
            s.live_on_device = Some(m.id);
            if let Some(f) = &m.frame {
                s.mirror = Anim { frames: Arc::new(vec![f.clone()]), speed: 1000, revision: s.mirror.revision + 1 };
            }
        }
    }
    s.mode = DisplayMode::Live;
    s.on_screen = title;
}

fn apply(s: &mut Snapshot, cmd: Command, timers: &mut Vec<(Instant, Command)>) {
    log(s, format!("{cmd:?}").chars().take(90).collect());
    match cmd {
        Command::SetPage(p) => s.page = p,
        Command::SetTheme(t) => s.theme = t,
        Command::Quit => s.quit = true,
        Command::Connect(on) => {
            if on {
                s.device.conn = Conn::Connecting;
                timers.push((Instant::now() + Duration::from_millis(1500), Command::Connect(true)));
            } else {
                s.device.conn = Conn::Disconnected;
            }
        }
        Command::SetBrightness(v) => s.device.brightness = v,
        Command::SetVolume(v) => s.device.volume = Some(v.min(15)),
        Command::PlayPause => s.device.playing = Some(!s.device.playing.unwrap_or(false)),
        Command::ScreenOnOff(on) => s.device.screen_on = on,
        Command::SetAwayEnabled(on) => s.device.away_enabled = on,
        Command::SetAwayBrightness(v) => s.device.away_brightness = v,
        Command::Discover => {
            s.device.discovering = true;
            timers.push((Instant::now() + Duration::from_secs(2), Command::Discover));
        }
        Command::SetMac(m) => s.settings.mac = m,
        Command::SetChannel(c) => s.settings.channel = c,
        Command::SetAutoConnect(b) => s.settings.auto_connect = b,
        Command::SetKeepalive(v) => s.settings.keepalive = v,
        Command::SetChunkDelay(v) => s.settings.chunk_delay = v,
        Command::SetZstdLevel(v) => s.settings.zstd_level = v,
        Command::SetCloseToTray(b) => s.settings.close_to_tray = b,
        Command::SetStartHidden(b) => s.settings.start_hidden = b,
        Command::SetFit(f) => s.image.fit = f,
        Command::SetPixelArt(b) => s.image.pixel_art = b,
        Command::SetCrop(r) => s.image.crop = r,
        Command::ResetCrop => {
            if let Some(src) = &s.image.source {
                s.image.crop = NRect::center_5x4(src.width as f64, src.height as f64);
            }
        }
        Command::SendImage => {
            s.mirror = Anim { frames: s.image.preview.frames.clone(), speed: s.image.preview.speed, revision: s.mirror.revision + 1 };
            s.mode = DisplayMode::Image;
            s.on_screen = "изображение".into();
            s.last_transfer = "12.0 КБ, 6 кадр., 800 мс".into();
            s.rotation.running = false;
            for m in &mut s.modes {
                m.on_device = false;
            }
        }
        Command::GalleryFilter(f) => s.image.filter = f,
        Command::GalleryFavorite(id, on) => {
            let mut g = (*s.image.gallery).clone();
            if let Some(it) = g.iter_mut().find(|g| g.id == id) {
                it.favorite = on;
            }
            s.image.gallery = Arc::new(g);
        }
        Command::GalleryRemove(id) => {
            let g: Vec<_> = s.image.gallery.iter().filter(|g| g.id != id).cloned().collect();
            s.image.gallery = Arc::new(g);
        }
        Command::GalleryOpen(id) => {
            if let Some(src) = s.image.source.as_mut() {
                src.id = id.clone();
                if let Some(g) = s.image.gallery.iter().find(|g| g.id == id) {
                    src.name = g.name.clone();
                }
            }
        }
        Command::GallerySend(id) => {
            if let Some(g) = s.image.gallery.iter().find(|g| g.id == id)
                && let Some(t) = &g.thumb
            {
                s.mirror = Anim { frames: Arc::new(vec![t.clone()]), speed: 1000, revision: s.mirror.revision + 1 };
                s.mode = DisplayMode::Image;
                s.on_screen = "изображение".into();
            }
        }
        Command::SetFolder(f) => s.image.folder = f,
        Command::AddPaths(p) | Command::OpenFiles(p) => {
            let mut g = (*s.image.gallery).clone();
            for (i, path) in p.iter().enumerate() {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                g.insert(0, GalleryItemView { id: format!("new{i}{}", g.len()), name: name.clone(), file: path.clone(), width: 64, height: 64, frames: 1, thumb: Some(thumb(0.6, "NEW")), ..Default::default() });
            }
            s.image.gallery = Arc::new(g);
        }
        Command::SelectSource | Command::StartCapture => {
            s.screen.status = CaptureStatus::Selecting;
            timers.push((Instant::now() + Duration::from_millis(1200), Command::StartCapture));
        }
        Command::StopCapture => {
            s.screen.status = CaptureStatus::Idle;
            s.screen.live = None;
            s.screen.streaming = false;
        }
        Command::SetRegion(r) => s.screen.region = r,
        Command::StartStream => {
            s.screen.streaming = true;
            s.mode = DisplayMode::Screen;
            s.on_screen = "трансляция экрана".into();
        }
        Command::StopStream => s.screen.streaming = false,
        Command::SetFps(f) => s.screen.fps = f,
        Command::SetCrisp(b) => s.screen.crisp = b,
        Command::SetQuality(q) => s.screen.quality = q,
        Command::ShowLive(id) => {
            s.rotation.running = false;
            show_live(s, &id);
        }
        Command::StopLive => {
            s.mode = DisplayMode::Idle;
            s.rotation.running = false;
            s.live_on_device = None;
            for m in &mut s.modes {
                m.on_device = false;
            }
            s.on_screen = "ничего (ожидание)".into();
        }
        Command::SetRotationMember(id, on) => {
            if let Some(m) = find_mode(s, &id) {
                m.in_rotation = on;
            }
            s.rotation.checked = s.modes.iter().filter(|m| m.in_rotation).count();
        }
        Command::StartRotation => {
            s.rotation.running = true;
            if let Some(first) = s.modes.iter().find(|m| m.in_rotation).map(|m| m.id) {
                show_live(s, first);
            }
        }
        Command::StopRotation => s.rotation.running = false,
        Command::RotationNext => s.rotation.progress = 1.0,
        Command::SetRotationInterval(v) => s.rotation.interval = v,
        Command::SetNotifyEnabled(b) => s.notify.enabled = b,
        Command::SetNotifyDuration(d) => s.notify.duration = d,
        Command::SetNotifyIgnore(v) => s.notify.ignore = v,
        Command::Mode(id, mc) => {
            if let Some(m) = find_mode(s, &id) {
                match (&mut m.view, mc) {
                    (ModeView::Clock(c), ModeCommand::ClockStyle(st)) => c.style = st,
                    (ModeView::Clock(c), ModeCommand::ClockSearch(q)) => {
                        c.results = if q.chars().count() >= 2 {
                            vec![
                                City { name: format!("{q}град"), region: "Россия, Центральный".into(), lat: 55.0, lon: 37.0 },
                                City { name: format!("{q}ск"), region: "Россия".into(), lat: 54.0, lon: 36.0 },
                            ]
                        } else {
                            vec![]
                        }
                    }
                    (ModeView::Clock(c), ModeCommand::ClockPickCity(city)) => {
                        m.status = format!("{}: 12°, малооблачно, обновлено 20:48", city.name);
                        c.city = Some(city);
                        c.results.clear();
                    }
                    (ModeView::Clock(c), ModeCommand::ClockClearCity) => {
                        c.city = None;
                        m.status = "укажите город для погоды".into();
                    }
                    (ModeView::Pomodoro(p), ModeCommand::PomodoroStartPause) => p.running = !p.running,
                    (ModeView::Pomodoro(p), ModeCommand::PomodoroWork(v)) => p.work_min = v,
                    (ModeView::Pomodoro(p), ModeCommand::PomodoroBreak(v)) => p.break_min = v,
                    (ModeView::Pomodoro(p), ModeCommand::PomodoroLong(v)) => p.long_min = v,
                    (ModeView::NowPlaying(n), ModeCommand::PlayPause) => n.playing = !n.playing,
                    (ModeView::Visualizer(v), ModeCommand::VisualizerStyle(st)) => v.style = st,
                    (ModeView::Github(g), ModeCommand::GithubRemove(r)) => g.repos.retain(|x| x.name != r),
                    (ModeView::Github(g), ModeCommand::GithubAdd(r)) => {
                        if !r.contains('/') {
                            g.add_error = Some("нужно owner/repo или ссылка на репозиторий".into());
                        } else if g.repos.len() >= 4 {
                            g.add_error = Some("на экране помещается 4 репозитория".into());
                        } else {
                            g.add_error = None;
                            g.repos.push(RepoView { name: r, state: RunState::Loading, detail: "загрузка…".into(), url: String::new() });
                        }
                    }
                    (ModeView::Github(g), ModeCommand::GithubToken(_)) => g.has_token = true,
                    (ModeView::Github(g), ModeCommand::GithubClearToken) => g.has_token = false,
                    _ => {}
                }
            }
        }
        Command::SetClaudeMode(on) => {
            s.claude.mode_on = on;
            if on {
                s.mode = DisplayMode::Claude;
                s.on_screen = "статус Claude".into();
                s.rotation.running = false;
                let st = s.claude.state;
                if let Some(set) = s.claude.scenes.iter().find(|x| x.state == st) {
                    s.mirror = set.anim.clone();
                }
            } else {
                s.mode = DisplayMode::Idle;
            }
        }
        Command::SetInterrupt(b) => s.claude.interrupt = b,
        Command::SetIdleAlerts(b) => s.claude.idle_alerts = b,
        Command::SetAlertCaption(b) => s.claude.alert_caption = b,
        Command::SetSceneMinutes(m) => s.claude.scene_minutes = m,
        Command::SetPort(p) => s.claude.port = p,
        Command::ClearSessions => {
            s.claude.sessions.clear();
            s.claude.state = ClaudeState::Chilling;
        }
        Command::PickScene(st, v) => {
            if let Some(set) = s.claude.scenes.iter_mut().find(|x| x.state == st) {
                set.current = v.clone();
                set.anim = scene_anim(st, &v, set.anim.revision + 1);
            }
        }
        Command::SetSceneEnabled(st, v, on) => {
            if let Some(set) = s.claude.scenes.iter_mut().find(|x| x.state == st) {
                set.off.retain(|x| *x != v);
                if !on {
                    set.off.push(v);
                }
            }
        }
        Command::SetCustomFace(st, p) => {
            if let Some(set) = s.claude.scenes.iter_mut().find(|x| x.state == st) {
                set.custom = p;
            }
        }
        Command::InstallHooks => {
            s.claude.hooks_installed = true;
            s.claude.hooks_message = Some((true, "Хуки установлены в ~/.claude/settings.json".into()));
        }
        Command::UninstallHooks => {
            s.claude.hooks_installed = false;
            s.claude.hooks_message = Some((true, "Хуки удалены".into()));
        }
        Command::ClearLog => s.log = Arc::new(vec![]),
        _ => {}
    }
}

/// Second stage of delayed commands.
fn finish(s: &mut Snapshot, cmd: Command) {
    match cmd {
        Command::Connect(true) => s.device.conn = Conn::Connected,
        Command::Discover => {
            s.device.discovering = false;
            s.device.discovered = vec![("Divoom MiniToo-Audio".into(), "B1:21:81:05:E2:65".into()), ("Pixoo-64".into(), "11:75:58:2C:94:0A".into())];
        }
        Command::StartCapture => {
            s.screen.status = CaptureStatus::Capturing;
            s.screen.has_token = true;
            s.screen.source_size = Some((1920, 1080));
            s.screen.region = NRect::center_5x4(1920.0, 1080.0);
        }
        _ => {}
    }
}

fn desktop(t: f32) -> Arc<image::RgbaImage> {
    let mut c = Canvas::new(960, 540);
    c.vgradient(0.0, 0.0, 960.0, 540.0, Color::hex(0x1d2b4a), Color::hex(0x4a6fa5));
    c.fill_round_rect(80.0, 60.0, 520.0, 360.0, 10.0, Color::hex(0xf2f2f2));
    c.fill_rect(80.0, 60.0, 520.0, 30.0, Color::hex(0x3c3f45));
    for i in 0..9 {
        c.fill_rect(110.0, 120.0 + i as f32 * 30.0, 300.0 + (i * 37 % 140) as f32, 10.0, Color::hex(0x9aa3b0));
    }
    c.fill_circle(760.0 + (t * 2.0).sin() * 60.0, 260.0, 50.0, Color::hex(0xee6b3d));
    c.fill_rect(0.0, 510.0, 960.0, 30.0, Color::hex(0x202226));
    Arc::new(c.to_image())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let shot = args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    if let Some(dir) = &shot {
        let _ = std::fs::create_dir_all(dir);
    }
    let debug = flag("--debug");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Command>();
    let mut snap = build();
    snap.debug = debug;
    let shared = Arc::new(CoreShared { snapshot: RwLock::new(Arc::new(snap.clone())), tx, repaint: RwLock::new(None) });
    let core = CoreHandle(shared.clone());

    let worker = core.clone();
    std::thread::spawn(move || {
        let mut s = snap;
        let mut timers: Vec<(Instant, Command)> = Vec::new();
        let start = Instant::now();
        let mut last_tick = Instant::now();
        loop {
            let mut changed = false;
            while let Ok(cmd) = rx.try_recv() {
                apply(&mut s, cmd, &mut timers);
                changed = true;
            }
            let now = Instant::now();
            let due: Vec<Command> = {
                let (d, rest): (Vec<_>, Vec<_>) = timers.drain(..).partition(|(t, _)| *t <= now);
                timers = rest;
                d.into_iter().map(|(_, c)| c).collect()
            };
            for c in due {
                finish(&mut s, c);
                changed = true;
            }
            if last_tick.elapsed() >= Duration::from_millis(250) {
                last_tick = Instant::now();
                let t = start.elapsed().as_secs_f32();
                if s.rotation.running && !s.rotation.paused {
                    s.rotation.progress += 0.25 / s.rotation.interval.max(1) as f32;
                    if s.rotation.progress >= 1.0 {
                        s.rotation.progress = 0.0;
                    }
                    s.rotation.seconds_left = ((1.0 - s.rotation.progress) * s.rotation.interval as f32).ceil() as u32;
                }
                if let Some(m) = find_mode(&mut s, "visualizer") {
                    m.frame = Some(visualizer_frame(t));
                    m.revision += 1;
                }
                if let Some(m) = find_mode(&mut s, "sysmon") {
                    let cpu = 0.1 + 0.3 * (t * 0.5).sin().abs();
                    m.frame = Some(sysmon_frame(cpu));
                    m.status = format!("CPU {}%  ·  RAM 3.9 ГБ", (cpu * 100.0) as i32);
                    m.revision += 1;
                }
                if s.screen.status == CaptureStatus::Capturing {
                    s.screen.live = Some(desktop(t));
                    s.screen.live_counter += 1;
                    if s.screen.streaming {
                        s.screen.actual_fps = s.screen.fps as f32 - 0.4;
                        s.screen.frame_kb = 8.1;
                    }
                }
                changed = true;
            }
            if changed {
                *worker.0.snapshot.write() = Arc::new(s.clone());
                worker.request_repaint();
                if s.quit {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    });

    let opts = UiOptions { start_hidden: flag("--hidden"), debug, screenshot_dir: shot };
    loop {
        match minitoo::ui::run(core.clone(), &opts) {
            UiExit::Quit => break,
            UiExit::Hidden => {
                eprintln!("window hidden; showing it again in 2 s");
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
}
