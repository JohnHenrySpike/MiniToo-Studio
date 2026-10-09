//! Сейчас играет (`nowplaying`, §8.4): cover and track of any MPRIS player.
//!
//! Polls the session bus once a second (async, never on the controller loop). While a track
//! plays, the device gets a whole minute of progress at once ("minute ahead"); a new batch goes
//! out on a new track, pause, seek, or when that minute runs out.

use super::{LiveMode, ModeCommand, ModeCx, ModeMsg, ModeView, NowPlayingView};
use crate::canvas::{r, Align, Canvas};
use crate::color::Color;
use crate::fonts::FontSpec;
use crate::frame::Frame;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TIMER_POLL: u64 = 1;
const TIMER_REPOLL: u64 = 2;
/// How long the device waits for a cover that is loading.
const ART_WAIT: Duration = Duration::from_millis(1500);

/// One player's state as read over MPRIS.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerInfo {
    /// bus name, `org.mpris.MediaPlayer2.<name>`
    pub service: String,
    pub identity: String,
    pub title: String,
    /// `xesam:artist` joined with ", "
    pub artist: String,
    pub album: String,
    /// µs
    pub length: i64,
    /// µs
    pub position: i64,
    /// `Playing` / `Paused` / `Stopped`
    pub status: String,
    pub art_url: String,
}

impl PlayerInfo {
    pub fn playing(&self) -> bool {
        self.status == "Playing"
    }
}

/// Fallback identity: the 4th part of the bus name (`org.mpris.MediaPlayer2.firefox.instance_1`
/// → `firefox`).
pub fn identity_from_service(service: &str) -> String {
    service.split('.').nth(3).unwrap_or("").to_string()
}

// ---------------------------------------------------------------------- pure logic

/// "Does the position move" with hysteresis: some players (Firefox) never update `Position`,
/// and others skip an update now and then.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Moves {
    pub stuck: u32,
    pub moves: bool,
}

impl Moves {
    pub fn update(&mut self, same_position: bool, playing: bool) {
        self.stuck = if same_position { self.stuck + 1 } else { 0 };
        if !playing || self.stuck >= 3 {
            self.moves = false;
        } else if self.stuck == 0 {
            self.moves = true;
        }
    }
}

/// Whether the device needs a new batch of frames.
///
/// * `key_changed`: track, status, cover, … differ from what was sent;
/// * `moving`: the batch sent plays progress;
/// * positions in µs, `elapsed_ms` since the batch was sent, `sent_seconds` frames in it.
pub fn should_send(key_changed: bool, moving: bool, position: i64, sent_position: i64, elapsed_ms: i64, sent_seconds: i64) -> bool {
    if key_changed {
        return true;
    }
    if moving {
        let expected = sent_position + elapsed_ms * 1000;
        (position - expected).abs() > 2_000_000 || elapsed_ms / 1000 >= sent_seconds - 1
    } else {
        position / 1_000_000 != sent_position / 1_000_000 // paused and moved
    }
}

/// Number of one-second frames for the device.
pub fn device_frame_count(moving: bool, length: i64, position: i64) -> usize {
    if !moving {
        return 1;
    }
    let left = if length > 0 { (length - position) / 1_000_000 } else { 0 };
    left.clamp(1, 60) as usize
}

/// `m:ss` of a µs value.
pub fn mmss(us: i64) -> String {
    let s = us.max(0) / 1_000_000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Local path of a `file://` URL (percent-decoded).
pub fn file_url_path(url: &str) -> Option<std::path::PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(out)))
    }
    #[cfg(not(unix))]
    {
        Some(std::path::PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
    }
}

/// Decodes a cover, drops alpha (over black) and caps its size.
fn prepare_art(img: image::DynamicImage) -> image::RgbaImage {
    let img = if img.width() > 1024 || img.height() > 1024 { img.resize(1024, 1024, image::imageops::FilterType::Triangle) } else { img };
    let mut rgba = img.to_rgba8();
    for p in rgba.pixels_mut() {
        let a = p.0[3] as u32;
        if a < 255 {
            p.0 = [(p.0[0] as u32 * a / 255) as u8, (p.0[1] as u32 * a / 255) as u8, (p.0[2] as u32 * a / 255) as u8, 255];
        }
    }
    rgba
}

/// Blurred cover over the whole screen, darkened, plus the cover itself in a rounded square.
pub fn art_layer(art: &image::RgbaImage) -> Canvas {
    use image::imageops::{resize, FilterType};
    let mut layer = Canvas::device();
    let (w, h) = (art.width().max(1), art.height().max(1));
    // cheap blur: downscale 16× and smooth upscale
    let small = resize(art, (w / 16).max(2), (h / 16).max(2), FilterType::Triangle);
    let blurred = resize(&small, 160, 128, FilterType::Triangle);
    layer.draw_canvas(&Canvas::from_image(&blurred), 0.0, 0.0, 1.0);
    layer.fill_rect(0.0, 0.0, 160.0, 128.0, Color::rgba(0, 0, 0, 150));
    // the cover, cropped to a square
    let side = w.min(h);
    let square = image::imageops::crop_imm(art, (w - side) / 2, (h - side) / 2, side, side).to_image();
    let cover = resize(&square, 62, 62, FilterType::CatmullRom);
    layer.draw_canvas_rounded(&Canvas::from_image(&cover), r(8.0, 8.0, 62.0, 62.0), 5.0);
    layer
}

// ---------------------------------------------------------------------- the mode

enum NpMsg {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Polled {
        #[cfg(target_os = "linux")]
        conn: Option<zbus::Connection>,
        result: Result<Option<PlayerInfo>, String>,
    },
    Art {
        url: String,
        image: Option<image::RgbaImage>,
    },
}

pub struct NowPlaying {
    p: PlayerInfo,
    art: Option<Arc<image::RgbaImage>>,
    /// changes with every loaded picture (0 = none), like `QImage::cacheKey`
    art_key: u64,
    next_art_key: u64,
    art_cache: Option<(u64, Canvas)>,
    /// a cover is loading: the device waits for it (a moment) instead of getting two batches
    art_pending: Option<Instant>,
    moves: Moves,
    // what the device got
    device_key: String,
    device_position: i64,
    device_clock: Option<Instant>,
    device_seconds: i64,
    // polling
    polling: bool,
    poll_again: bool,
    error: Option<String>,
    #[cfg(target_os = "linux")]
    conn: Option<zbus::Connection>,
}

impl Default for NowPlaying {
    fn default() -> Self {
        Self::new()
    }
}

impl NowPlaying {
    pub fn new() -> Self {
        NowPlaying {
            p: PlayerInfo::default(),
            art: None,
            art_key: 0,
            next_art_key: 0,
            art_cache: None,
            art_pending: None,
            moves: Moves::default(),
            device_key: String::new(),
            device_position: 0,
            device_clock: None,
            device_seconds: 0,
            polling: false,
            poll_again: false,
            error: None,
            #[cfg(target_os = "linux")]
            conn: None,
        }
    }

    fn playing(&self) -> bool {
        self.p.playing()
    }

    fn moving(&self) -> bool {
        self.playing() && self.moves.moves
    }

    /// «исполнитель — трек»
    fn track(&self) -> String {
        if self.p.artist.is_empty() { self.p.title.clone() } else { format!("{} — {}", self.p.artist, self.p.title) }
    }

    fn status_text(&self) -> String {
        if let Some(e) = &self.error {
            return e.clone();
        }
        if self.p.service.is_empty() { tr!("nowplaying.no_player").into() } else { format!("{}: {}", self.p.identity, self.track()) }
    }

    fn poll(&mut self, cx: &mut ModeCx) {
        #[cfg(target_os = "linux")]
        {
            if self.polling {
                self.poll_again = true;
                return;
            }
            self.polling = true;
            let conn = self.conn.clone();
            cx.spawn(async move {
                let (conn, result) = mpris::poll(conn).await;
                Box::new(NpMsg::Polled { conn, result }) as ModeMsg
            });
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (cx, &mut self.polling, &mut self.poll_again);
        }
    }

    /// Applies a poll (`NowPlayingMode::poll` of the reference).
    fn apply(&mut self, cx: &mut ModeCx, chosen: Option<PlayerInfo>) {
        match chosen {
            None => {
                let (length, position) = (self.p.length, self.p.position);
                self.p = PlayerInfo { length, position, ..Default::default() };
                self.art = None;
                self.art_key = 0;
                self.art_pending = None;
            }
            Some(n) => {
                let previous = self.p.position;
                let art_url = std::mem::take(&mut self.p.art_url);
                self.p = PlayerInfo { art_url, ..n.clone() };
                self.moves.update(self.p.position == previous, self.playing());
                if self.p.identity.is_empty() {
                    self.p.identity = identity_from_service(&self.p.service);
                }
                if n.art_url != self.p.art_url {
                    self.load_art(cx, n.art_url);
                }
            }
        }
        cx.set_status(self.status_text());
        self.render(cx);
    }

    fn load_art(&mut self, cx: &mut ModeCx, url: String) {
        self.p.art_url = url.clone();
        self.art = None;
        self.art_key = 0;
        self.art_pending = None;
        if url.is_empty() {
            return;
        }
        if let Some(path) = file_url_path(&url) {
            self.art_pending = Some(Instant::now());
            cx.spawn_blocking(move || {
                let image = image::open(&path).ok().map(prepare_art);
                Box::new(NpMsg::Art { url, image }) as ModeMsg
            });
        } else if url.starts_with("http://") || url.starts_with("https://") {
            self.art_pending = Some(Instant::now());
            let http = cx.http().clone();
            cx.spawn(async move {
                let bytes = async {
                    let resp = http.get(&url).timeout(Duration::from_secs(15)).send().await.ok()?;
                    resp.error_for_status().ok()?.bytes().await.ok()
                }
                .await;
                let image = match bytes {
                    Some(b) => tokio::task::spawn_blocking(move || image::load_from_memory(&b).ok().map(prepare_art))
                        .await
                        .ok()
                        .flatten(),
                    None => None,
                };
                Box::new(NpMsg::Art { url, image }) as ModeMsg
            });
        }
    }

    /// The frame for a track position (µs).
    pub fn render_at(&mut self, position: i64) -> Frame {
        let mut c = Canvas::device();
        c.fill(Color::rgb(14, 12, 22));
        let p = &self.p;

        if p.service.is_empty() || p.title.is_empty() {
            let grey = Color::rgb(60, 60, 80);
            c.fill_ellipse(70.0 - 9.0, 58.0 - 7.0, 18.0, 14.0, grey);
            c.fill_rect(77.0, 30.0, 3.0, 28.0, grey);
            c.text(r(0.0, 76.0, 160.0, 20.0), Align::CENTER, tr!("nowplaying.nothing_playing"), FontSpec::bold(12.0), Color::rgb(170, 170, 190));
            return c.to_frame();
        }

        if let Some(art) = &self.art {
            if self.art_cache.as_ref().is_none_or(|(k, _)| *k != self.art_key) {
                self.art_cache = Some((self.art_key, art_layer(art)));
            }
            if let Some((_, layer)) = &self.art_cache {
                c.draw_canvas(layer, 0.0, 0.0, 1.0);
            }
        } else {
            c.fill_round_rect(8.0, 8.0, 62.0, 62.0, 5.0, Color::rgb(50, 40, 80));
            c.fill_ellipse(36.0 - 8.0, 48.0 - 6.0, 16.0, 12.0, Color::ACCENT);
            c.fill_rect(42.0, 20.0, 3.0, 28.0, Color::ACCENT);
        }

        // title may wrap to two lines
        let tf = FontSpec::bold(12.0);
        let title = Canvas::elide(&p.title, tf, 150.0);
        c.text_wrapped(r(76.0, 8.0, 80.0, 34.0), Align::TOP_LEFT, &title, tf, Color::WHITE, 2, 0.0);
        let af = FontSpec::sans(10.0);
        c.text(r(76.0, 42.0, 80.0, 14.0), Align::LEFT, &Canvas::elide(&p.artist, af, 80.0), af, Color::rgb(205, 205, 220));
        let pf = FontSpec::sans(9.0);
        c.text(r(76.0, 56.0, 80.0, 12.0), Align::LEFT, &Canvas::elide(&p.identity, pf, 80.0), pf, Color::rgb(140, 220, 150));

        let pos = if p.length > 0 { position.min(p.length) } else { position };
        let frac = if p.length > 0 { pos as f64 / p.length as f64 } else { 0.0 };
        c.bar(8.0, 84.0, 144.0, 4.0, frac as f32, Color::ACCENT, Some(Color::rgb(70, 70, 90)));
        let tc = Color::rgb(200, 200, 210);
        c.text(r(8.0, 90.0, 50.0, 12.0), Align::LEFT, &mmss(pos), pf, tc);
        if p.length > 0 {
            c.text(r(102.0, 90.0, 50.0, 12.0), Align::RIGHT, &mmss(p.length), pf, tc);
        }

        // transport icons: triangles with their tip at (x, y)
        let tri = |c: &mut Canvas, x: f32, y: f32, right: bool| {
            let d = if right { -8.0 } else { 8.0 };
            c.fill_polygon(&[(x, y), (x + d, y - 5.0), (x + d, y + 5.0)], Color::WHITE);
        };
        tri(&mut c, 42.0, 112.0, false);
        tri(&mut c, 50.0, 112.0, false);
        if p.playing() {
            c.fill_rect(74.0, 106.0, 4.0, 12.0, Color::WHITE);
            c.fill_rect(82.0, 106.0, 4.0, 12.0, Color::WHITE);
        } else {
            c.fill_polygon(&[(75.0, 105.0), (87.0, 112.0), (75.0, 119.0)], Color::WHITE);
        }
        tri(&mut c, 110.0, 112.0, true);
        tri(&mut c, 118.0, 112.0, true);
        c.to_frame()
    }

    fn device_key_now(&self) -> String {
        let p = &self.p;
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            p.service,
            p.title,
            p.artist,
            p.identity,
            p.status,
            self.art_key,
            p.length,
            self.moving()
        )
    }

    /// Sets the state directly (snapshots, tests, examples).
    pub fn set_state(&mut self, info: PlayerInfo, art: Option<image::RgbaImage>, moves: bool) {
        self.p = info;
        self.moves = Moves { stuck: 0, moves };
        self.art = art.map(|a| Arc::new(prepare_art(image::DynamicImage::ImageRgba8(a))));
        self.next_art_key += 1;
        self.art_key = if self.art.is_some() { self.next_art_key } else { 0 };
    }

    /// Frames the device would get (`device_frames` without a running mode).
    pub fn frames_ahead(&mut self) -> Vec<Frame> {
        let count = device_frame_count(self.moving(), self.p.length, self.p.position);
        self.device_position = self.p.position;
        self.device_clock = Some(Instant::now());
        self.device_seconds = count as i64;
        let pos = self.p.position;
        (0..count).map(|i| self.render_at(pos + i as i64 * 1_000_000)).collect()
    }
}

impl LiveMode for NowPlaying {
    fn id(&self) -> &'static str {
        "nowplaying"
    }
    fn title(&self) -> &'static str {
        tr!("nowplaying.title")
    }
    fn subtitle(&self) -> &'static str {
        tr!("nowplaying.subtitle")
    }
    fn icon(&self) -> &'static str {
        "music"
    }
    fn owns_device_signal(&self) -> bool {
        true
    }

    fn start(&mut self, cx: &mut ModeCx) {
        self.polling = false;
        self.poll_again = false;
        self.device_key.clear();
        if self.art.is_none() {
            // a load cut short by the last stop: load it again with the next poll
            self.p.art_url.clear();
            self.art_pending = None;
        }
        #[cfg(target_os = "linux")]
        {
            self.poll(cx);
            cx.timer(Duration::from_secs(1), TIMER_POLL);
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.error = Some(tr!("nowplaying.err.unsupported").into());
            cx.set_status(self.status_text());
        }
    }

    fn stop(&mut self, _cx: &mut ModeCx) {
        self.polling = false;
        self.poll_again = false;
    }

    fn render(&mut self, cx: &mut ModeCx) {
        let frame = self.render_at(self.p.position);
        cx.publish(frame);
        if self.art_pending.is_some_and(|t| t.elapsed() < ART_WAIT) {
            return; // the cover comes in a moment: one batch instead of two
        }
        let key = self.device_key_now();
        let elapsed_ms = self.device_clock.map(|t| t.elapsed().as_millis() as i64).unwrap_or(0);
        if should_send(
            key != self.device_key,
            self.moving(),
            self.p.position,
            self.device_position,
            elapsed_ms,
            self.device_seconds,
        ) {
            self.device_key = key;
            // also guards against a burst of resends before device_frames() runs
            self.device_position = self.p.position;
            self.device_clock = Some(Instant::now());
            cx.device_frames_changed();
        }
    }

    fn on_timer(&mut self, cx: &mut ModeCx, token: u64) {
        match token {
            TIMER_POLL => {
                self.poll(cx);
                cx.timer(Duration::from_secs(1), TIMER_POLL);
            }
            TIMER_REPOLL => self.poll(cx),
            _ => {}
        }
    }

    fn on_message(&mut self, cx: &mut ModeCx, msg: ModeMsg) {
        let Ok(msg) = msg.downcast::<NpMsg>() else { return };
        match *msg {
            #[allow(unused_variables)]
            NpMsg::Polled {
                #[cfg(target_os = "linux")]
                conn,
                result,
            } => {
                #[cfg(target_os = "linux")]
                if conn.is_some() {
                    self.conn = conn;
                }
                self.polling = false;
                match result {
                    Ok(chosen) => {
                        self.error = None;
                        self.apply(cx, chosen);
                    }
                    Err(e) => {
                        if self.error.as_ref() != Some(&e) {
                            cx.log(format!("nowplaying: {e}"));
                        }
                        self.error = Some(e);
                        self.apply(cx, None);
                    }
                }
                if std::mem::take(&mut self.poll_again) {
                    self.poll(cx);
                }
            }
            NpMsg::Art { url, image } => {
                if url == self.p.art_url {
                    self.art_pending = None;
                    if let Some(img) = image {
                        self.art = Some(Arc::new(img));
                        self.next_art_key += 1;
                        self.art_key = self.next_art_key;
                    }
                    self.render(cx);
                }
            }
        }
    }

    fn device_frames(&mut self, _cx: &mut ModeCx) -> Option<(Vec<Frame>, u32)> {
        Some((self.frames_ahead(), 1000))
    }

    fn command(&mut self, cx: &mut ModeCx, cmd: ModeCommand) {
        let method = match cmd {
            ModeCommand::PlayPause => "PlayPause",
            ModeCommand::Next => "Next",
            ModeCommand::Previous => "Previous",
            _ => return,
        };
        if self.p.service.is_empty() {
            return;
        }
        #[cfg(target_os = "linux")]
        if let Some(conn) = self.conn.clone() {
            let service = self.p.service.clone();
            cx.spawn(async move {
                mpris::control(&conn, &service, method).await;
                Box::new(()) as ModeMsg
            });
            cx.timer(Duration::from_millis(300), TIMER_REPOLL);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (cx, method);
    }

    fn view(&self) -> ModeView {
        ModeView::NowPlaying(NowPlayingView {
            available: !self.p.service.is_empty(),
            player: self.p.identity.clone(),
            artist: self.p.artist.clone(),
            title: self.p.title.clone(),
            playing: self.playing(),
        })
    }
}

// ---------------------------------------------------------------------- MPRIS over D-Bus

#[cfg(target_os = "linux")]
pub mod mpris {
    //! The few MPRIS calls the mode needs, on the session bus.

    use super::PlayerInfo;
    use std::collections::HashMap;
    use std::time::Duration;
    use zbus::zvariant::{OwnedValue, Value};

    pub const PATH: &str = "/org/mpris/MediaPlayer2";
    pub const PREFIX: &str = "org.mpris.MediaPlayer2.";
    const ROOT_IFACE: &str = "org.mpris.MediaPlayer2";
    const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";

    fn v_str(v: &Value) -> String {
        match v {
            Value::Str(s) => s.to_string(),
            Value::ObjectPath(p) => p.to_string(),
            Value::Value(b) => v_str(b),
            _ => String::new(),
        }
    }

    fn v_i64(v: &Value) -> i64 {
        match v {
            Value::I64(x) => *x,
            Value::U64(x) => *x as i64,
            Value::I32(x) => *x as i64,
            Value::U32(x) => *x as i64,
            Value::I16(x) => *x as i64,
            Value::U16(x) => *x as i64,
            Value::U8(x) => *x as i64,
            Value::F64(x) => *x as i64,
            Value::Value(b) => v_i64(b),
            _ => 0,
        }
    }

    fn v_strs(v: &Value) -> Vec<String> {
        match v {
            Value::Array(a) => a.inner().iter().map(v_str).filter(|s| !s.is_empty()).collect(),
            Value::Str(s) => vec![s.to_string()],
            Value::Value(b) => v_strs(b),
            _ => Vec::new(),
        }
    }

    fn v_dict<'a>(v: &'a Value<'a>) -> Vec<(String, &'a Value<'a>)> {
        match v {
            Value::Dict(d) => d.iter().map(|(k, v)| (v_str(k), v)).collect(),
            Value::Value(b) => v_dict(b),
            _ => Vec::new(),
        }
    }

    /// `Properties.GetAll(iface)` with a 500 ms timeout; empty on any failure.
    pub async fn get_all(conn: &zbus::Connection, service: &str, iface: &str) -> HashMap<String, OwnedValue> {
        let body = (iface,);
        let call = conn.call_method(Some(service), PATH, Some("org.freedesktop.DBus.Properties"), "GetAll", &body);
        match tokio::time::timeout(Duration::from_millis(500), call).await {
            Ok(Ok(reply)) => reply.body().deserialize::<HashMap<String, OwnedValue>>().unwrap_or_default(),
            _ => HashMap::new(),
        }
    }

    /// Player state from the `org.mpris.MediaPlayer2.Player` properties (no identity yet).
    pub fn parse_player(service: &str, props: &HashMap<String, OwnedValue>) -> PlayerInfo {
        let mut p = PlayerInfo { service: service.to_string(), ..Default::default() };
        if let Some(meta) = props.get("Metadata") {
            for (k, v) in v_dict(meta) {
                match k.as_str() {
                    "xesam:title" => p.title = v_str(v),
                    "xesam:artist" => p.artist = v_strs(v).join(", "),
                    "xesam:album" => p.album = v_str(v),
                    "mpris:length" => p.length = v_i64(v),
                    "mpris:artUrl" => p.art_url = v_str(v),
                    _ => {}
                }
            }
        }
        if let Some(s) = props.get("PlaybackStatus") {
            p.status = v_str(s);
        }
        if let Some(pos) = props.get("Position") {
            p.position = v_i64(pos);
        }
        p
    }

    pub async fn session() -> Result<zbus::Connection, String> {
        zbus::Connection::session().await.map_err(|e| tr!("nowplaying.err.no_session_bus", error = e))
    }

    /// Every MPRIS player on the bus with its state (`None`: it did not answer), in bus order.
    pub async fn players(conn: &zbus::Connection) -> Result<Vec<(String, Option<PlayerInfo>)>, String> {
        let call = conn.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "ListNames", &());
        let reply = tokio::time::timeout(Duration::from_secs(2), call)
            .await
            .map_err(|_| tr!("nowplaying.err.dbus_timeout").to_string())?
            .map_err(|e| format!("D-Bus: {e}"))?;
        let names: Vec<String> = reply.body().deserialize().map_err(|e| format!("D-Bus: {e}"))?;
        let names: Vec<String> = names.into_iter().filter(|n| n.starts_with(PREFIX)).collect();
        let all = futures_util::future::join_all(names.iter().map(|n| get_all(conn, n, PLAYER_IFACE))).await;
        Ok(names
            .into_iter()
            .zip(all)
            .map(|(n, props)| {
                let info = if props.is_empty() { None } else { Some(parse_player(&n, &props)) };
                (n, info)
            })
            .collect())
    }

    /// `Identity` of the player (fallback: the 4th part of the bus name).
    pub async fn identity(conn: &zbus::Connection, service: &str) -> String {
        let props = get_all(conn, service, ROOT_IFACE).await;
        let id = props.get("Identity").map(|v| v_str(v)).unwrap_or_default();
        if id.is_empty() { super::identity_from_service(service) } else { id }
    }

    /// The player to show: the first one playing, otherwise the first with data.
    pub fn choose(players: Vec<(String, Option<PlayerInfo>)>) -> Option<PlayerInfo> {
        let with_data: Vec<PlayerInfo> = players.into_iter().filter_map(|(_, p)| p).collect();
        let playing = with_data.iter().position(|p| p.playing());
        with_data.into_iter().nth(playing.unwrap_or(0))
    }

    /// One poll; hands the connection back so it is reused.
    pub async fn poll(conn: Option<zbus::Connection>) -> (Option<zbus::Connection>, Result<Option<PlayerInfo>, String>) {
        let conn = match conn {
            Some(c) => c,
            None => match session().await {
                Ok(c) => c,
                Err(e) => return (None, Err(e)),
            },
        };
        let result = match players(&conn).await {
            Err(e) => Err(e),
            Ok(list) => match choose(list) {
                None => Ok(None),
                Some(mut p) => {
                    p.identity = identity(&conn, &p.service).await;
                    Ok(Some(p))
                }
            },
        };
        (Some(conn), result)
    }

    /// `PlayPause` / `Next` / `Previous` without waiting for an answer.
    pub async fn control(conn: &zbus::Connection, service: &str, method: &str) {
        let msg = zbus::Message::method_call(PATH, method)
            .and_then(|b| b.destination(service))
            .and_then(|b| b.interface(PLAYER_IFACE))
            .and_then(|b| b.with_flags(zbus::message::Flags::NoReplyExpected))
            .and_then(|b| b.build(&()));
        if let Ok(msg) = msg {
            let _ = conn.send(&msg).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_hysteresis() {
        let mut m = Moves::default();
        m.update(false, true);
        assert!(m.moves);
        // a skipped update or two keeps it moving
        m.update(true, true);
        m.update(true, true);
        assert!(m.moves);
        // the third unchanged poll in a row: stuck
        m.update(true, true);
        assert!(!m.moves);
        m.update(true, true);
        assert!(!m.moves);
        // moving again at once
        m.update(false, true);
        assert!(m.moves);
        // not playing: never moves
        m.update(false, false);
        assert!(!m.moves);
        // after a pause, one unchanged poll does not switch it back on
        m.update(true, true);
        assert!(!m.moves);
        m.update(false, true);
        assert!(m.moves);
    }

    #[test]
    fn send_decision() {
        let s = 1_000_000;
        // key change always sends
        assert!(should_send(true, false, 0, 0, 0, 1));
        // moving: on schedule, nothing to send
        assert!(!should_send(false, true, 10 * s + 5 * s, 10 * s, 5000, 60));
        // moving: seek by more than 2 s
        assert!(should_send(false, true, 10 * s + 8 * s, 10 * s, 5000, 60));
        assert!(should_send(false, true, 10 * s, 10 * s, 5000, 60)); // backwards / stuck by 5 s
        assert!(!should_send(false, true, 10 * s + 3 * s, 10 * s, 5000, 60)); // 2 s off: tolerated
        // moving: the last second of the sent minute
        assert!(!should_send(false, true, 10 * s + 58 * s, 10 * s, 58_000, 60));
        assert!(should_send(false, true, 10 * s + 59 * s, 10 * s, 59_000, 60));
        // a short batch (track end): its last second
        assert!(should_send(false, true, 10 * s + 4 * s, 10 * s, 4000, 5));
        // standing: when the second of the position changes
        assert!(!should_send(false, false, 10 * s + 900_000, 10 * s, 30_000, 1));
        assert!(should_send(false, false, 11 * s, 10 * s, 30_000, 1));
    }

    #[test]
    fn frame_counts() {
        let s = 1_000_000;
        assert_eq!(device_frame_count(false, 200 * s, 10 * s), 1);
        assert_eq!(device_frame_count(true, 200 * s, 10 * s), 60);
        assert_eq!(device_frame_count(true, 200 * s, 190 * s), 10);
        assert_eq!(device_frame_count(true, 200 * s, 200 * s), 1);
        assert_eq!(device_frame_count(true, 0, 10 * s), 1); // unknown length
        assert_eq!(device_frame_count(true, 200 * s, 199 * s + 500_000), 1);

        let mut np = NowPlaying::new();
        let info = PlayerInfo {
            service: "org.mpris.MediaPlayer2.test".into(),
            identity: "Test".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            length: 200 * s,
            position: 185 * s,
            status: "Playing".into(),
            ..Default::default()
        };
        np.set_state(info.clone(), None, true);
        let frames = np.frames_ahead();
        assert_eq!(frames.len(), 15);
        assert_ne!(frames[0], frames[1]); // the clock moves
        np.set_state(PlayerInfo { status: "Paused".into(), ..info }, None, true);
        assert_eq!(np.frames_ahead().len(), 1);
    }

    #[test]
    fn helpers() {
        assert_eq!(mmss(0), "0:00");
        assert_eq!(mmss(61_500_000), "1:01");
        assert_eq!(mmss(-5), "0:00");
        assert_eq!(identity_from_service("org.mpris.MediaPlayer2.firefox.instance_1_23"), "firefox");
        assert_eq!(identity_from_service("org.mpris.MediaPlayer2.vlc"), "vlc");
        assert_eq!(
            file_url_path("file:///home/u/Music/A%20B/cover%2Ejpg"),
            Some(std::path::PathBuf::from("/home/u/Music/A B/cover.jpg"))
        );
        assert_eq!(file_url_path("file:///x/100%"), Some(std::path::PathBuf::from("/x/100%")));
        assert_eq!(file_url_path("https://x/y.jpg"), None);
    }

    #[test]
    fn draws_frames() {
        let mut np = NowPlaying::new();
        let empty = np.render_at(0);
        assert_eq!(empty.pixel(0, 0), [14, 12, 22]);
        let art = image::RgbaImage::from_fn(300, 200, |x, _| image::Rgba([(x % 256) as u8, 40, 200, 255]));
        np.set_state(
            PlayerInfo {
                service: "org.mpris.MediaPlayer2.test".into(),
                identity: "Test".into(),
                title: "A rather long song title that wraps".into(),
                artist: "Artist".into(),
                length: 200_000_000,
                position: 50_000_000,
                status: "Playing".into(),
                ..Default::default()
            },
            Some(art),
            true,
        );
        let f = np.render_at(50_000_000);
        assert_ne!(f, empty);
        // pause bars are white, the progress bar starts with the accent
        assert_eq!(f.pixel(75, 110), [255, 255, 255]);
        assert_eq!(f.pixel(10, 85), [0xd9, 0x77, 0x57]);
    }
}
