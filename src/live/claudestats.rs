//! Статистика Claude (`claudestats`, §8.6): today's tokens from Claude Code transcripts
//! (`~/.claude/projects/**/*.jsonl`, read incrementally every 30 s), the live sessions and the
//! subscription limits (`api/oauth/usage` with Claude Code's login, every 3 min), in one of
//! six screen styles.

use super::{ClaudeStatsView, LiveMode, ModeCommand, ModeCx, ModeMsg, ModeView};
use crate::canvas::{r, Align, Canvas, LineCap};
use crate::claude::{ClaudeState, Session};
use crate::color::Color;
use crate::fonts::FontSpec;
use crate::frame::Frame;
use chrono::{DateTime, Local, NaiveDate};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

const SCAN: u64 = 1;
const LIMITS: u64 = 2;
const EVERY: Duration = Duration::from_secs(30);
const LIMITS_EVERY: Duration = Duration::from_secs(180);
/// after a 429 the interval doubles, up to this
const LIMITS_MAX: Duration = Duration::from_secs(15 * 60);

/// Today's totals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub replies: i64,
    pub prompts: i64,
}

impl Totals {
    pub fn all(&self) -> i64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// Incremental transcript reader: per-file offsets and the assistant message ids seen today.
#[derive(Debug, Default)]
pub struct Scanner {
    day: Option<NaiveDate>,
    offsets: HashMap<PathBuf, u64>,
    seen: HashSet<String>,
    pub totals: Totals,
}

impl Scanner {
    /// Reads what was appended to today's `*.jsonl` under `root` since the last scan.
    pub fn scan(&mut self, root: &Path, today: NaiveDate) {
        if self.day != Some(today) {
            *self = Scanner { day: Some(today), ..Default::default() };
        }
        let Some(day_start) = today.and_hms_opt(0, 0, 0).and_then(|d| d.and_local_timezone(Local).earliest()) else { return };
        let mut files = Vec::new();
        collect_jsonl(root, &mut files, 0);
        for path in files {
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let modified: Option<DateTime<Local>> = meta.modified().ok().map(DateTime::from);
            if modified.is_none_or(|m| m < day_start) {
                continue;
            }
            let offset = self.offsets.get(&path).copied().unwrap_or(0);
            if meta.len() <= offset {
                continue;
            }
            let Ok(mut f) = std::fs::File::open(&path) else { continue };
            if f.seek(SeekFrom::Start(offset)).is_err() {
                continue;
            }
            let mut buf = Vec::new();
            if f.read_to_end(&mut buf).is_err() {
                continue;
            }
            let mut consumed = 0usize;
            while let Some(nl) = buf[consumed..].iter().position(|&b| b == b'\n') {
                let line = &buf[consumed..consumed + nl];
                consumed += nl + 1;
                self.line(line, today);
            }
            // a partial last line is read again next time
            self.offsets.insert(path, offset + consumed as u64);
        }
    }

    fn line(&mut self, line: &[u8], today: NaiveDate) {
        if !contains(line, b"\"timestamp\"") {
            return;
        }
        let Ok(o) = serde_json::from_slice::<serde_json::Value>(line) else { return };
        let Some(ts) = o.get("timestamp").and_then(|v| v.as_str()) else { return };
        let Ok(ts) = DateTime::parse_from_rfc3339(ts) else { return };
        if ts.with_timezone(&Local).date_naive() != today {
            return;
        }
        let msg = o.get("message");
        match o.get("type").and_then(|v| v.as_str()) {
            Some("user") => {
                if msg.and_then(|m| m.get("content")).is_some_and(|c| c.is_string()) {
                    self.totals.prompts += 1;
                }
            }
            Some("assistant") => {
                let Some(msg) = msg else { return };
                let id = msg.get("id").and_then(|v| v.as_str()).unwrap_or("");
                // streamed parts repeat the same usage
                if id.is_empty() || !self.seen.insert(id.to_string()) {
                    return;
                }
                let u = msg.get("usage");
                let n = |k: &str| u.and_then(|u| u.get(k)).and_then(|v| v.as_i64()).unwrap_or(0);
                self.totals.input += n("input_tokens");
                self.totals.output += n("output_tokens");
                self.totals.cache_read += n("cache_read_input_tokens");
                self.totals.cache_write += n("cache_creation_input_tokens");
                self.totals.replies += 1;
            }
            _ => {}
        }
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 8 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            collect_jsonl(&p, out, depth + 1);
        } else if ft.is_file() && p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

/// ≥1M → «12.3M» (two decimals below 10M), ≥1000 → «4.5K».
pub fn compact(n: i64) -> String {
    if n >= 1_000_000 {
        let decimals = if n >= 10_000_000 { 1 } else { 2 };
        format!("{:.*}M", decimals, n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

pub fn summary(t: &Totals) -> String {
    tr!(
        "claudestats.summary",
        total = compact(t.all()),
        input = compact(t.input),
        output = compact(t.output),
        cache = compact(t.cache_read + t.cache_write),
        replies = t.replies,
        prompts = t.prompts
    )
}

// ---------------------------------------------------------------------- plan limits

/// One usage window of the subscription: the 5-hour session or the week.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limit {
    /// 0..100
    pub pct: f32,
    pub resets_at: Option<DateTime<Local>>,
}

/// The plan limits from `api/oauth/usage`; `None` = unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    pub session: Option<Limit>,
    pub week: Option<Limit>,
}

impl Limits {
    pub fn known(&self) -> bool {
        self.session.is_some() || self.week.is_some()
    }
}

/// Parses an `api/oauth/usage` reply: `five_hour` / `seven_day` with `utilization` (percent)
/// and `resets_at` (RFC 3339).
pub fn parse_limits(v: &serde_json::Value) -> Limits {
    let window = |k: &str| {
        let o = v.get(k)?;
        let pct = o.get("utilization")?.as_f64()? as f32;
        let resets_at = o
            .get("resets_at")
            .and_then(|x| x.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Local));
        Some(Limit { pct, resets_at })
    };
    Limits { session: window("five_hour"), week: window("seven_day") }
}

/// Why there is no usable login.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoLogin {
    /// no file, or no token in it (never signed in, signed out, or the CLI dropped a login it
    /// could not renew)
    Missing,
    /// the token expired: `claude` renews it on its next start while its refresh token lives
    Expired,
    /// the server refused the token
    Refused,
}

/// The Claude Code CLI login: `claudeAiOauth.accessToken` of `.credentials.json` in
/// `$CLAUDE_CONFIG_DIR` or `~/.claude`. Claude Desktop keeps its own login and never writes
/// this file, so with only the desktop app in use the CLI login runs out.
pub fn oauth_token(now_ms: i64) -> Result<String, NoLogin> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".claude")));
    let v = dir
        .and_then(|d| std::fs::read_to_string(d.join(".credentials.json")).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .ok_or(NoLogin::Missing)?;
    token_from(&v, now_ms)
}

fn token_from(v: &serde_json::Value, now_ms: i64) -> Result<String, NoLogin> {
    let o = v.get("claudeAiOauth").ok_or(NoLogin::Missing)?;
    let token = o.get("accessToken").and_then(|t| t.as_str()).filter(|t| !t.is_empty()).ok_or(NoLogin::Missing)?;
    // the server would refuse it anyway
    if o.get("expiresAt").and_then(|x| x.as_i64()).is_some_and(|exp| exp <= now_ms) {
        return Err(NoLogin::Expired);
    }
    Ok(token.to_string())
}

enum LimitsReply {
    Ok(Limits),
    NoLogin(NoLogin),
    /// 429: the endpoint wants fewer requests
    TooMany,
    Failed(String),
}

async fn fetch_limits(http: reqwest::Client) -> LimitsReply {
    let token = match oauth_token(Local::now().timestamp_millis()) {
        Ok(t) => t,
        Err(why) => return LimitsReply::NoLogin(why),
    };
    let req = http
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "minitoo-studio")
        .timeout(Duration::from_secs(20));
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return LimitsReply::Failed(e.without_url().to_string()),
    };
    let code = resp.status();
    if code == reqwest::StatusCode::UNAUTHORIZED || code == reqwest::StatusCode::FORBIDDEN {
        return LimitsReply::NoLogin(NoLogin::Refused);
    }
    if code == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return LimitsReply::TooMany;
    }
    if !code.is_success() {
        return LimitsReply::Failed(format!("HTTP {}", code.as_u16()));
    }
    match resp.json::<serde_json::Value>().await {
        Ok(v) => {
            let l = parse_limits(&v);
            if l.known() { LimitsReply::Ok(l) } else { LimitsReply::Failed(tr!("claudestats.limits_format").to_string()) }
        }
        Err(e) => LimitsReply::Failed(e.without_url().to_string()),
    }
}

/// A limit as drawn at `now`: a passed reset means a fresh window (0%).
#[derive(Clone, Debug, PartialEq)]
pub struct Gauge {
    pub pct: f32,
    /// share of the window already elapsed (where an even pace would be), if the reset is known
    pub pace: Option<f32>,
    /// «4ч 38м» / «сегодня 19:00»
    pub reset: String,
}

const HOUR: i64 = 3600;

fn gauge(l: &Limit, window_secs: i64, countdown: bool, now: DateTime<Local>) -> Gauge {
    let Some(at) = l.resets_at else { return Gauge { pct: l.pct, pace: None, reset: String::new() } };
    let left = (at - now).num_seconds();
    if left <= 0 {
        return Gauge { pct: 0.0, pace: None, reset: String::new() };
    }
    let pace = Some((1.0 - left as f32 / window_secs as f32).clamp(0.0, 1.0));
    let reset = if countdown { countdown_text(left) } else { when_text(at, now) };
    Gauge { pct: l.pct, pace, reset }
}

/// «4ч 38м», «12м».
pub fn countdown_text(secs: i64) -> String {
    let mins = (secs + 59) / 60;
    if mins >= 60 { tr!("claudestats.left_hm", h = mins / 60, m = mins % 60) } else { tr!("claudestats.left_m", m = mins) }
}

/// «сегодня 19:00», «завтра 19:00», «чт 19:00».
pub fn when_text(at: DateTime<Local>, now: DateTime<Local>) -> String {
    let time = crate::i18n::time_hm(&at);
    let days = (at.date_naive() - now.date_naive()).num_days();
    match days {
        0 => tr!("claudestats.today_at", time = time),
        1 => tr!("claudestats.tomorrow_at", time = time),
        _ => tr!("claudestats.day_at", day = crate::i18n::weekday_short(&at), time = time),
    }
}

/// Everything a screen shows.
pub struct View<'a> {
    pub sessions: &'a [Session],
    /// running subagents in all sessions
    pub agents: usize,
    pub totals: &'a Totals,
    pub session: Option<Gauge>,
    pub week: Option<Gauge>,
}

impl<'a> View<'a> {
    pub fn new(sessions: &'a [Session], totals: &'a Totals, limits: &Limits, now: DateTime<Local>) -> Self {
        View {
            sessions,
            agents: sessions.iter().map(|s| s.agents.len()).sum(),
            totals,
            session: limits.session.map(|l| gauge(&l, 5 * HOUR, true, now)),
            week: limits.week.map(|l| gauge(&l, 7 * 24 * HOUR, false, now)),
        }
    }
}

// ---------------------------------------------------------------------- drawing

/// Screen styles, in the order of the picker.
pub const STYLES: [&str; 6] = [
    "modes.claudestats.style_bars",
    "modes.claudestats.style_rings",
    "modes.claudestats.style_big",
    "modes.claudestats.style_side",
    "modes.claudestats.style_pixels",
    "modes.claudestats.style_classic",
];

const BG: Color = Color::rgb(18, 15, 22);
const DIM: Color = Color::rgb(150, 145, 165);
const HEAD: Color = Color::rgb(30, 18, 14);
const TRACK: Color = Color::rgb(42, 37, 51);
const RULE: Color = Color::rgb(55, 50, 65);
const AMBER: Color = Color::rgb(240, 170, 60);
const RED: Color = Color::rgb(230, 64, 48);

fn state_color(s: ClaudeState) -> Color {
    match s {
        ClaudeState::Alerting => RED,
        ClaudeState::Working => Color::ACCENT,
        ClaudeState::Chilling => Color::rgb(90, 120, 190),
    }
}

/// Accent; amber from 70% or well ahead of an even pace; red from 90%.
fn severity(g: &Gauge) -> Color {
    if g.pct >= 90.0 {
        RED
    } else if g.pct >= 70.0 || g.pace.is_some_and(|p| g.pct / 100.0 > p + 0.15) {
        AMBER
    } else {
        Color::ACCENT
    }
}

fn pct_text(g: Option<&Gauge>) -> String {
    g.map(|g| format!("{}%", g.pct.round() as i64)).unwrap_or_else(|| "—".into())
}

fn gauge_color(g: Option<&Gauge>) -> Color {
    g.map(severity).unwrap_or(DIM)
}

fn reset_text(g: Option<&Gauge>) -> &str {
    g.map(|g| g.reset.as_str()).unwrap_or("")
}

/// Horizontal bar with the pace tick.
fn pace_bar(c: &mut Canvas, x: f32, y: f32, w: f32, h: f32, g: Option<&Gauge>) {
    match g {
        Some(g) => {
            c.bar(x, y, w, h, g.pct / 100.0, severity(g), Some(TRACK));
            if let Some(p) = g.pace {
                c.fill_rect((x + w * p).round(), y - 2.0, 1.0, h + 4.0, Color::WHITE);
            }
        }
        None => c.fill_round_rect(x, y, w, h, h / 2.0, TRACK),
    }
}

/// Accent header; `right` (the session count) on the right, the agents badge left of it.
fn header(c: &mut Canvas, h: f32, right: &str, agents: usize) {
    c.fill_rect(0.0, 0.0, 160.0, h, Color::ACCENT);
    let font = FontSpec::bold(11.0);
    c.text(r(6.0, 0.0, 110.0, h), Align::LEFT, "Claude Code", font, HEAD);
    c.text(r(90.0, 0.0, 64.0, h), Align::RIGHT, right, font, HEAD);
    let gap = if right.is_empty() { 0.0 } else { Canvas::text_width(right, font) + 5.0 };
    agent_badge_right(c, 154.0 - gap, h / 2.0, agents, true);
}

const BADGE_FONT: FontSpec = FontSpec::bold(8.0);
use crate::faces::AGENT_GLYPH;
const GLYPH_W: f32 = 7.0;

/// Width of the agents badge; 0 without agents.
fn badge_width(n: usize) -> f32 {
    if n == 0 { 0.0 } else { (4.0 + GLYPH_W + 2.0 + Canvas::text_width(&n.to_string(), BADGE_FONT) + 4.0).round() }
}

/// «[glyph] N» pill with its left edge at `x`, centred on `cy`. `on_accent`: dark pill for
/// the accent header, otherwise an accent-tinted one for the dark background.
fn agent_badge(c: &mut Canvas, x: f32, cy: f32, n: usize, on_accent: bool) {
    if n == 0 {
        return;
    }
    let w = badge_width(n);
    let (bg, fg) = if on_accent { (HEAD, Color::rgb(250, 200, 180)) } else { (Color::rgb(70, 42, 34), Color::rgb(245, 170, 140)) };
    let (x, top) = (x.round(), (cy - 5.5).round());
    c.fill_round_rect(x, top, w, 11.0, 5.5, bg);
    for (ry, row) in AGENT_GLYPH.iter().enumerate() {
        for (rx, ch) in row.chars().enumerate() {
            if ch == '#' {
                c.set_pixel(x as i32 + 4 + rx as i32, top as i32 + 3 + ry as i32, fg);
            }
        }
    }
    let tx = x + 4.0 + GLYPH_W + 2.0;
    c.text(r(tx, top - 1.0, w, 13.0), Align::LEFT, &n.to_string(), BADGE_FONT, fg);
}

/// The badge ending at `right`.
fn agent_badge_right(c: &mut Canvas, right: f32, cy: f32, n: usize, on_accent: bool) {
    agent_badge(c, right - badge_width(n), cy, n, on_accent);
}

/// Sessions as dots from `right_x` leftwards (at most `max`). Returns the left edge drawn.
fn session_dots(c: &mut Canvas, right_x: f32, cy: f32, sessions: &[Session], max: usize, ring: Option<Color>) -> f32 {
    let mut x = right_x;
    for s in sessions.iter().take(max) {
        if let Some(o) = ring {
            c.fill_circle(x, cy, 4.0, o);
        }
        c.fill_circle(x, cy, 3.0, state_color(s.state));
        x -= 9.0;
    }
    x + 9.0 - 4.0
}

/// Session rows: dot, project, optionally the state on the right.
#[allow(clippy::too_many_arguments)]
fn session_rows(c: &mut Canvas, x: f32, y0: f32, w: f32, row: f32, max: usize, sessions: &[Session], states: bool) {
    if sessions.is_empty() {
        c.text(r(x, y0, w, row), Align::LEFT, tr!("claudestats.no_sessions"), FontSpec::sans(9.0), DIM);
        return;
    }
    let mut y = y0;
    for s in sessions.iter().take(max) {
        let color = state_color(s.state);
        c.fill_circle(x + 3.0, y + row / 2.0, 3.0, color);
        let nf = FontSpec::bold(10.0);
        let bw = badge_width(s.agents.len());
        let nw = if states { w * 0.6 } else { w - 10.0 } - if bw > 0.0 { bw + 4.0 } else { 0.0 };
        let name = Canvas::elide(&s.project(), nf, nw);
        c.text(r(x + 10.0, y, nw + 2.0, row), Align::LEFT, &name, nf, Color::WHITE);
        agent_badge(c, x + 10.0 + Canvas::text_width(&name, nf) + 4.0, y + row / 2.0, s.agents.len(), false);
        if states {
            c.text(r(x, y, w, row), Align::RIGHT, s.state.lower(), FontSpec::sans(9.0), color);
        }
        y += row;
    }
}

/// The statistics screen in `style` (see [`STYLES`]).
pub fn frame(style: i64, v: &View) -> Frame {
    let mut c = Canvas::device();
    match style {
        1 => draw_rings(&mut c, v),
        2 => draw_big(&mut c, v),
        3 => draw_side(&mut c, v),
        4 => draw_pixels(&mut c, v),
        5 => draw_classic(&mut c, v),
        _ => draw_bars(&mut c, v),
    }
    c.to_frame()
}

/// «Полосы»: two sessions, both limits as bars, today's tokens.
fn draw_bars(c: &mut Canvas, v: &View) {
    c.fill(BG);
    header(c, 18.0, &v.sessions.len().to_string(), v.agents);
    session_rows(c, 6.0, 20.0, 148.0, 15.0, 2, v.sessions, true);
    c.line(6.0, 52.0, 154.0, 52.0, 1.0, RULE);
    for (i, (label, g)) in [(tr!("claudestats.session"), v.session.as_ref()), (tr!("claudestats.week"), v.week.as_ref())].into_iter().enumerate() {
        let y = 55.0 + i as f32 * 27.0;
        let pf = FontSpec::bold(12.0);
        let p = pct_text(g);
        let pw = Canvas::text_width(&p, pf);
        c.text(r(6.0, y, 60.0, 14.0), Align::LEFT, &p, pf, gauge_color(g));
        c.text(r(10.0 + pw, y, 60.0, 14.0), Align::LEFT, label, FontSpec::sans(9.0), DIM);
        c.text(r(70.0, y, 84.0, 14.0), Align::RIGHT, reset_text(g), FontSpec::sans(9.0), DIM);
        pace_bar(c, 6.0, y + 16.0, 148.0, 5.0, g);
    }
    c.line(6.0, 108.0, 154.0, 108.0, 1.0, RULE);
    let t = v.totals;
    c.text(r(6.0, 110.0, 80.0, 16.0), Align::LEFT, &tr!("claudestats.tokens_short", total = compact(t.all())), FontSpec::bold(10.0), Color::WHITE);
    c.text(r(70.0, 110.0, 84.0, 16.0), Align::RIGHT, &tr!("claudestats.today_replies", count = t.replies), FontSpec::sans(8.0), DIM);
}

fn ring(c: &mut Canvas, cx: f32, cy: f32, g: Option<&Gauge>, label: &str) {
    let rad = 19.0;
    c.arc(cx, cy, rad, 225.0, -270.0, 6.0, TRACK, LineCap::Round);
    if let Some(g) = g {
        c.arc(cx, cy, rad, 225.0, -270.0 * (g.pct / 100.0).clamp(0.0, 1.0), 6.0, severity(g), LineCap::Round);
        if let Some(p) = g.pace {
            let a = (225.0 - 270.0 * p).to_radians();
            let (o, i) = (rad + 5.0, rad - 5.0);
            c.line(cx + i * a.cos(), cy - i * a.sin(), cx + o * a.cos(), cy - o * a.sin(), 1.5, Color::WHITE);
        }
    }
    c.text(r(cx - 22.0, cy - 9.0, 44.0, 18.0), Align::CENTER, &pct_text(g), FontSpec::bold(12.0), Color::WHITE);
    c.text(r(cx - 30.0, cy + 20.0, 60.0, 12.0), Align::CENTER, label, FontSpec::sans(9.0), DIM);
    c.text(r(cx - 39.0, cy + 32.0, 78.0, 12.0), Align::CENTER, reset_text(g), FontSpec::bold(9.0), Color::rgb(210, 205, 220));
}

/// «Кольца»: two ring gauges, sessions as dots in the header.
fn draw_rings(c: &mut Canvas, v: &View) {
    c.fill(BG);
    header(c, 18.0, "", 0);
    // the badge and the dots share the room right of the title
    let title_end = 6.0 + Canvas::text_width("Claude Code", FontSpec::bold(11.0)) + 4.0;
    let bw = badge_width(v.agents);
    let room = 156.0 - title_end - if bw > 0.0 { bw + 4.0 } else { 0.0 };
    let left = session_dots(c, 152.0, 9.0, v.sessions, ((room / 9.0) as usize).min(6), Some(HEAD));
    agent_badge_right(c, left - 4.0, 9.0, v.agents, true);
    ring(c, 40.0, 46.0, v.session.as_ref(), tr!("claudestats.five_hours"));
    ring(c, 120.0, 46.0, v.week.as_ref(), tr!("claudestats.week"));
    c.line(6.0, 108.0, 154.0, 108.0, 1.0, RULE);
    c.text(r(6.0, 110.0, 60.0, 16.0), Align::LEFT, tr!("claudestats.today"), FontSpec::sans(9.0), DIM);
    c.text(r(50.0, 110.0, 104.0, 16.0), Align::RIGHT, &tr!("claudestats.tokens_short", total = compact(v.totals.all())), FontSpec::bold(11.0), Color::WHITE);
}

/// «Крупно»: the session limit in big digits, the week below.
fn draw_big(c: &mut Canvas, v: &View) {
    c.fill(BG);
    c.text(r(6.0, 2.0, 100.0, 14.0), Align::LEFT, tr!("claudestats.five_hour_limit"), FontSpec::sans(9.0), DIM);
    session_dots(c, 152.0, 9.0, v.sessions, 5, None);
    let s = v.session.as_ref();
    c.text(r(4.0, 14.0, 92.0, 40.0), Align::LEFT, &pct_text(s), FontSpec::bold(36.0), gauge_color(s));
    if s.is_some_and(|g| !g.reset.is_empty()) {
        c.text(r(92.0, 20.0, 62.0, 12.0), Align::RIGHT, tr!("claudestats.resets_in"), FontSpec::sans(8.0), DIM);
        c.text(r(80.0, 32.0, 74.0, 16.0), Align::RIGHT, reset_text(s), FontSpec::bold(13.0), Color::WHITE);
    }
    pace_bar(c, 6.0, 57.0, 148.0, 8.0, s);

    let w = v.week.as_ref();
    let label = tr!("claudestats.week");
    let lf = FontSpec::sans(9.0);
    c.text(r(6.0, 72.0, 60.0, 14.0), Align::LEFT, label, lf, DIM);
    c.text(r(10.0 + Canvas::text_width(label, lf), 72.0, 40.0, 14.0), Align::LEFT, &pct_text(w), FontSpec::bold(11.0), gauge_color(w));
    c.text(r(80.0, 72.0, 74.0, 14.0), Align::RIGHT, reset_text(w), lf, DIM);
    pace_bar(c, 6.0, 88.0, 148.0, 4.0, w);

    c.fill_rect(0.0, 104.0, 160.0, 24.0, Color::rgb(30, 26, 36));
    let who = trn!("claudestats.sessions", v.sessions.len());
    let wf = FontSpec::sans(9.0);
    c.text(r(6.0, 104.0, 70.0, 24.0), Align::LEFT, &who, wf, DIM);
    agent_badge(c, 6.0 + Canvas::text_width(&who, wf) + 5.0, 116.0, v.agents, false);
    c.text(r(60.0, 104.0, 94.0, 24.0), Align::RIGHT, &tr!("claudestats.tokens_short", total = compact(v.totals.all())), FontSpec::bold(11.0), Color::WHITE);
}

fn column(c: &mut Canvas, x: f32, top: f32, h: f32, w: f32, g: Option<&Gauge>, label: &str) {
    c.fill_round_rect(x, top, w, h, 3.0, TRACK);
    if let Some(g) = g {
        let fh = (h * g.pct / 100.0).clamp(0.0, h);
        if fh > 0.0 {
            c.fill_round_rect(x, top + h - fh, w, fh, 3.0, severity(g));
        }
        if let Some(p) = g.pace {
            c.fill_rect(x - 2.0, (top + h - h * p).round(), w + 4.0, 1.0, Color::WHITE);
        }
    }
    c.text(r(x - 6.0, top + h + 2.0, w + 12.0, 12.0), Align::CENTER, &pct_text(g), FontSpec::bold(8.0), Color::WHITE);
    let lw = crate::pixelfont::width(label, 0);
    c.pixel_text((x + w / 2.0) as i32 - lw / 2, (top + h + 13.0) as i32, label, DIM, 0, 1);
}

/// «Сбоку»: the session list as before, two vertical columns on the right.
fn draw_side(c: &mut Canvas, v: &View) {
    c.fill(BG);
    header(c, 18.0, &v.sessions.len().to_string(), v.agents);
    session_rows(c, 6.0, 22.0, 100.0, 16.0, 4, v.sessions, false);
    c.line(6.0, 92.0, 106.0, 92.0, 1.0, RULE);
    c.text(r(6.0, 94.0, 60.0, 14.0), Align::LEFT, tr!("claudestats.today"), FontSpec::sans(9.0), DIM);
    c.text(r(6.0, 107.0, 104.0, 18.0), Align::LEFT, &tr!("claudestats.tokens_short", total = compact(v.totals.all())), FontSpec::bold(13.0), Color::WHITE);
    c.line(112.0, 22.0, 112.0, 124.0, 1.0, RULE);
    column(c, 120.0, 26.0, 72.0, 10.0, v.session.as_ref(), tr!("claudestats.col_session"));
    column(c, 142.0, 26.0, 72.0, 10.0, v.week.as_ref(), tr!("claudestats.col_week"));
}

/// A 24-segment LED meter (green, amber from 65%, red from 85%) with the pace caret below.
fn led_bar(c: &mut Canvas, x: i32, y: i32, g: Option<&Gauge>) {
    const SEGS: i32 = 24;
    let lit = g.map(|g| (g.pct / 100.0 * SEGS as f32).round() as i32).unwrap_or(0);
    for i in 0..SEGS {
        let f = i as f32 / SEGS as f32;
        let on = if f >= 0.85 {
            RED
        } else if f >= 0.65 {
            AMBER
        } else {
            Color::rgb(90, 200, 120)
        };
        c.fill_rect((x + i * 6) as f32, y as f32, 5.0, 9.0, if i < lit { on } else { on.with_alpha(40) });
    }
    if let Some(p) = g.and_then(|g| g.pace) {
        let px = x + (p * (SEGS * 6) as f32) as i32;
        c.fill_rect(px as f32 - 1.0, (y + 11) as f32, 3.0, 1.0, Color::WHITE);
        c.fill_rect(px as f32, (y + 10) as f32, 1.0, 1.0, Color::WHITE);
    }
}

/// «Пиксели»: LED meters in the 5×7 pixel font.
fn draw_pixels(c: &mut Canvas, v: &View) {
    c.fill(Color::rgb(8, 8, 10));
    c.pixel_text(8, 3, "CLAUDE CODE", Color::ACCENT, 1, 1);
    let mut x = 150;
    for s in v.sessions.iter().take(8) {
        c.fill_rect(x as f32, 5.0, 5.0, 5.0, state_color(s.state));
        x -= 8;
    }
    // agents: the little glyph and the count, left of the squares
    if v.agents > 0 {
        let n = v.agents.to_string();
        let right = x + 8 - 5;
        let nx = right - crate::pixelfont::width(&n, 1);
        c.pixel_text(nx, 3, &n, Color::ACCENT, 1, 1);
        let gx = nx - GLYPH_W as i32 - 2;
        for (ry, row) in AGENT_GLYPH.iter().enumerate() {
            for (rx, ch) in row.chars().enumerate() {
                if ch == '#' {
                    c.set_pixel(gx + rx as i32, 5 + ry as i32, Color::ACCENT);
                }
            }
        }
    }
    let rows = [(tr!("claudestats.session_5h"), v.session.as_ref()), (tr!("claudestats.week"), v.week.as_ref())];
    for (i, (label, g)) in rows.into_iter().enumerate() {
        let y = 20 + i as i32 * 42;
        c.pixel_text(8, y, &label.to_uppercase(), DIM, 1, 1);
        let p = pct_text(g);
        let p = if p == "—" { "-".to_string() } else { p };
        c.pixel_text(152 - crate::pixelfont::width(&p, 1) * 2, y - 6, &p, Color::WHITE, 1, 2);
        led_bar(c, 8, y + 14, g);
        if let Some(g) = g.filter(|g| !g.reset.is_empty()) {
            c.pixel_text(8, y + 27, &tr!("claudestats.reset", when = g.reset).to_uppercase(), Color::rgb(110, 105, 120), 0, 1);
        }
    }
    c.fill_rect(0.0, 112.0, 160.0, 1.0, Color::rgb(40, 38, 46));
    let today = format!("{} {}", tr!("claudestats.today"), tr!("claudestats.tokens_short", total = compact(v.totals.all())));
    c.pixel_text(8, 115, &today.to_uppercase(), Color::rgb(200, 195, 210), 1, 1);
}

/// «Классика»: sessions and today's tokens, no limits.
fn draw_classic(c: &mut Canvas, v: &View) {
    let (sessions, t) = (v.sessions, v.totals);
    c.fill(BG);
    c.fill_rect(0.0, 0.0, 160.0, 20.0, Color::ACCENT);
    c.text(r(8.0, 0.0, 120.0, 20.0), Align::LEFT, "Claude Code", FontSpec::bold(12.0), HEAD);
    let count = sessions.len().to_string();
    c.text(r(110.0, 0.0, 42.0, 20.0), Align::RIGHT, &count, FontSpec::bold(12.0), HEAD);
    agent_badge_right(c, 152.0 - Canvas::text_width(&count, FontSpec::bold(12.0)) - 5.0, 10.0, v.agents, true);

    let mut y = 26.0;
    if sessions.is_empty() {
        c.text(r(8.0, y, 144.0, 18.0), Align::LEFT, tr!("claudestats.no_sessions"), FontSpec::sans(10.0), DIM);
    }
    for s in sessions.iter().take(3) {
        let color = state_color(s.state);
        c.fill_circle(12.0, y + 9.0, 4.0, color);
        let name_font = FontSpec::bold(11.0);
        let bw = badge_width(s.agents.len());
        let name = Canvas::elide(&s.project(), name_font, 78.0 - if bw > 0.0 { bw + 4.0 } else { 0.0 });
        c.text(r(22.0, y, 80.0, 18.0), Align::LEFT, &name, name_font, Color::WHITE);
        agent_badge(c, 22.0 + Canvas::text_width(&name, name_font) + 4.0, y + 9.0, s.agents.len(), false);
        c.text(r(96.0, y, 56.0, 18.0), Align::RIGHT, s.state.lower(), FontSpec::sans(10.0), color);
        y += 20.0;
    }
    if sessions.len() > 3 {
        c.text(r(22.0, y - 4.0, 130.0, 10.0), Align::LEFT, &tr!("claudestats.more", count = sessions.len() - 3), FontSpec::sans(8.0), DIM);
    }

    c.line(8.0, 90.0, 152.0, 90.0, 1.0, RULE);
    c.text(r(8.0, 94.0, 60.0, 16.0), Align::LEFT, tr!("claudestats.today"), FontSpec::sans(9.0), DIM);
    c.text(r(50.0, 92.0, 102.0, 18.0), Align::RIGHT, &tr!("claudestats.tokens_short", total = compact(t.all())), FontSpec::bold(13.0), Color::WHITE);
    c.text(r(8.0, 111.0, 80.0, 14.0), Align::LEFT, &tr!("claudestats.replies", count = t.replies), FontSpec::sans(9.0), DIM);
    c.text(r(72.0, 111.0, 80.0, 14.0), Align::RIGHT, &tr!("claudestats.output", output = compact(t.output)), FontSpec::sans(9.0), DIM);
}

// ---------------------------------------------------------------------- the mode

struct ScanMsg(Scanner);

pub struct ClaudeStats {
    /// `None` while a scan runs on the blocking pool
    scanner: Option<Scanner>,
    totals: Totals,
    style: i64,
    limits: Limits,
    /// why the limits are missing (no subscription login, an HTTP error)
    limits_error: Option<String>,
    fetching: bool,
    /// the polling interval, longer after a 429
    every: Duration,
    /// the next request is scheduled (a style change fetches too and must not start a second chain)
    timer_pending: bool,
    loaded: bool,
}

impl Default for ClaudeStats {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeStats {
    pub fn new() -> Self {
        ClaudeStats {
            scanner: Some(Scanner::default()),
            totals: Totals::default(),
            style: 0,
            limits: Limits::default(),
            limits_error: None,
            fetching: false,
            every: LIMITS_EVERY,
            timer_pending: false,
            loaded: false,
        }
    }

    fn ensure_loaded(&mut self, cx: &ModeCx) {
        if !self.loaded {
            self.loaded = true;
            self.style = cx.settings.int_in("claudestats/style", 0, 0, STYLES.len() as i64 - 1);
        }
    }

    fn scan(&mut self, cx: &mut ModeCx) {
        let Some(mut scanner) = self.scanner.take() else { return };
        let today = cx.now().date_naive();
        cx.spawn_blocking(move || {
            if let Some(home) = dirs::home_dir() {
                scanner.scan(&home.join(".claude").join("projects"), today);
            }
            Box::new(ScanMsg(scanner)) as ModeMsg
        });
    }

    /// One request; its reply schedules the next.
    fn fetch(&mut self, cx: &mut ModeCx) {
        // the classic screen does not show them
        if self.fetching || self.style == CLASSIC {
            return;
        }
        self.fetching = true;
        let http = cx.http().clone();
        cx.spawn(async move { Box::new(fetch_limits(http).await) as ModeMsg });
    }

    fn status(&self, now: DateTime<Local>) -> String {
        let mut s = summary(&self.totals);
        if self.style != CLASSIC {
            let v = View::new(&[], &self.totals, &self.limits, now);
            if self.limits.known() {
                s += &tr!("claudestats.limits_status", session = pct_text(v.session.as_ref()), week = pct_text(v.week.as_ref()));
            }
        }
        s
    }
}

const CLASSIC: i64 = 5;

fn no_login_text(why: NoLogin) -> &'static str {
    match why {
        NoLogin::Missing => tr!("claudestats.limits_no_login"),
        NoLogin::Expired => tr!("claudestats.limits_expired"),
        NoLogin::Refused => tr!("claudestats.limits_refused"),
    }
}

impl LiveMode for ClaudeStats {
    fn id(&self) -> &'static str {
        "claudestats"
    }
    fn title(&self) -> &'static str {
        tr!("claudestats.title")
    }
    fn subtitle(&self) -> &'static str {
        tr!("claudestats.subtitle")
    }
    fn icon(&self) -> &'static str {
        "sparkle"
    }

    fn start(&mut self, cx: &mut ModeCx) {
        self.ensure_loaded(cx);
        // a scan or a request still running when the mode stopped never came back: start over
        if self.scanner.is_none() {
            self.scanner = Some(Scanner::default());
        }
        self.fetching = false;
        self.timer_pending = false;
        self.scan(cx);
        self.fetch(cx);
        cx.timer(EVERY, SCAN);
    }

    fn render(&mut self, cx: &mut ModeCx) {
        let now = cx.now();
        cx.publish(frame(self.style, &View::new(cx.sessions, &self.totals, &self.limits, now)));
    }

    fn on_timer(&mut self, cx: &mut ModeCx, token: u64) {
        match token {
            SCAN => {
                self.scan(cx);
                cx.timer(EVERY, SCAN);
            }
            LIMITS => {
                self.timer_pending = false;
                self.fetch(cx);
            }
            _ => {}
        }
    }

    fn on_message(&mut self, cx: &mut ModeCx, msg: ModeMsg) {
        let msg = match msg.downcast::<ScanMsg>() {
            Ok(m) => {
                let ScanMsg(scanner) = *m;
                self.totals = scanner.totals;
                self.scanner = Some(scanner);
                let status = self.status(cx.now());
                cx.set_status(status);
                // also moves the reset countdown on
                self.render(cx);
                return;
            }
            Err(m) => m,
        };
        let Ok(reply) = msg.downcast::<LimitsReply>() else { return };
        self.fetching = false;
        let previous = std::mem::replace(&mut self.every, LIMITS_EVERY);
        match *reply {
            LimitsReply::Ok(l) => {
                self.limits = l;
                self.limits_error = None;
            }
            LimitsReply::NoLogin(why) => {
                self.limits = Limits::default();
                self.limits_error = Some(no_login_text(why).to_string());
            }
            // the last known values stay until the window resets
            LimitsReply::TooMany => {
                self.every = (previous * 2).min(LIMITS_MAX);
                self.limits_error = Some(tr!("claudestats.limits_too_many", min = self.every.as_secs() / 60));
            }
            LimitsReply::Failed(e) => self.limits_error = Some(tr!("claudestats.limits_failed", error = e)),
        }
        if !self.timer_pending {
            self.timer_pending = true;
            cx.timer(self.every, LIMITS);
        }
        let status = self.status(cx.now());
        cx.set_status(status);
        self.render(cx);
    }

    fn on_sessions_changed(&mut self, cx: &mut ModeCx) {
        self.render(cx);
    }

    fn command(&mut self, cx: &mut ModeCx, cmd: ModeCommand) {
        self.ensure_loaded(cx);
        // timers and requests only run while the mode is acquired; the host drops them otherwise
        if let ModeCommand::ClaudeStatsStyle(s) = cmd {
            let s = s.clamp(0, STYLES.len() as i64 - 1);
            if s != self.style {
                self.style = s;
                cx.settings.set_int("claudestats/style", s);
                self.fetch(cx);
                let status = self.status(cx.now());
                cx.set_status(status);
                self.render(cx);
            }
        }
    }

    fn view(&self) -> ModeView {
        ModeView::ClaudeStats(ClaudeStatsView { style: self.style, limits_error: self.limits_error.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::io::Write;

    #[test]
    fn number_format() {
        assert_eq!(compact(999), "999");
        assert_eq!(compact(4500), "4.5K");
        assert_eq!(compact(4300), "4.3K");
        assert_eq!(compact(1_566_210), "1.57M");
        assert_eq!(compact(12_345_678), "12.3M");
        assert_eq!(compact(0), "0");
    }

    #[test]
    fn reads_jsonl_incrementally() {
        let root = std::env::temp_dir().join(format!("minitoo-claudestats-{}", std::process::id()));
        let dir = root.join("-home-x-proj").join("sub");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        let now = Local::now();
        let today = now.date_naive();
        let ts = now.to_utc().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"user","timestamp":"{ts}","message":{{"role":"user","content":"привет"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"user","timestamp":"{ts}","message":{{"content":[{{"type":"tool_result"}}]}}}}"#).unwrap();
        let a1 = format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"m1","usage":{{"input_tokens":1200,"output_tokens":3400,"cache_read_input_tokens":1500000,"cache_creation_input_tokens":45000}}}}}}"#
        );
        writeln!(f, "{a1}").unwrap();
        writeln!(f, "{a1}").unwrap();
        writeln!(f, r#"{{"type":"assistant","timestamp":"2020-01-01T00:00:00Z","message":{{"id":"old","usage":{{"output_tokens":5}}}}}}"#)
            .unwrap();
        write!(f, r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"m2","usage":{{"output_tokens":"#).unwrap();
        f.flush().unwrap();

        let mut s = Scanner::default();
        s.scan(&root, today);
        assert_eq!(s.totals, Totals { input: 1200, output: 3400, cache_read: 1_500_000, cache_write: 45000, replies: 1, prompts: 1 });
        // the partial line is finished later
        writeln!(f, r#"7}}}}}}"#).unwrap();
        f.flush().unwrap();
        s.scan(&root, today);
        assert_eq!(s.totals.output, 3407);
        assert_eq!(s.totals.replies, 2);
        // nothing new: nothing changes
        s.scan(&root, today);
        assert_eq!(s.totals.replies, 2);
        // a new day resets everything (today's file is then read from the start again)
        s.scan(&root, today.succ_opt().unwrap());
        assert_eq!(s.totals, Totals::default());
        assert!(summary(&Totals { input: 1200, output: 3400, ..Default::default() }).starts_with("сегодня: 4.6K токенов (вход 1.2K"));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn limit_reply() -> serde_json::Value {
        serde_json::json!({
            "five_hour": { "utilization": 14.0, "resets_at": "2026-10-13T13:10:00.452338+00:00" },
            "seven_day": { "utilization": 56.0, "resets_at": "2026-10-13T14:00:00+00:00" },
            "seven_day_opus": null
        })
    }

    #[test]
    fn parses_limits() {
        let l = parse_limits(&limit_reply());
        let s = l.session.unwrap();
        assert_eq!(s.pct, 14.0);
        assert_eq!(s.resets_at.unwrap().to_utc().to_rfc3339(), "2026-10-13T13:10:00.452338+00:00");
        assert_eq!(l.week.unwrap().pct, 56.0);
        assert!(!parse_limits(&serde_json::json!({ "five_hour": null })).known());
    }

    #[test]
    fn reads_token() {
        let v = serde_json::json!({ "claudeAiOauth": { "accessToken": "sk-ant-oat01-x", "expiresAt": 2000 } });
        assert_eq!(token_from(&v, 1000).as_deref(), Ok("sk-ant-oat01-x"));
        assert_eq!(token_from(&v, 2000), Err(NoLogin::Expired));
        // what the CLI leaves after a login it could not renew
        let wiped = serde_json::json!({ "claudeAiOauth": { "accessToken": "", "refreshToken": "", "expiresAt": 0 } });
        assert_eq!(token_from(&wiped, 0), Err(NoLogin::Missing));
        assert_eq!(token_from(&serde_json::json!({}), 0), Err(NoLogin::Missing));
    }

    #[test]
    fn gauges_at_a_moment() {
        crate::i18n::set_language("ru");
        crate::i18n::set_formats("24", "auto");
        let now = Local.with_ymd_and_hms(2026, 10, 13, 13, 32, 0).unwrap();
        let l = Limit { pct: 14.0, resets_at: Some(now + chrono::Duration::minutes(4 * 60 + 38)) };
        let g = gauge(&l, 5 * HOUR, true, now);
        assert_eq!(g.reset, "4ч 38м");
        assert!((g.pace.unwrap() - 22.0 / 300.0).abs() < 1e-4);
        // a passed reset is a fresh window
        let g = gauge(&l, 5 * HOUR, true, now + chrono::Duration::hours(5));
        assert_eq!((g.pct, g.pace, g.reset.as_str()), (0.0, None, ""));
        assert_eq!(countdown_text(59), "1м");
        assert_eq!(countdown_text(3600), "1ч 0м");
        let at = |d: u32, h: u32| Local.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap();
        assert_eq!(when_text(at(13, 19), now), "сегодня 19:00");
        assert_eq!(when_text(at(14, 9), now), "завтра 09:00");
        assert_eq!(when_text(at(15, 19), now), "чт 19:00");
    }

    #[test]
    fn draws_every_style() {
        crate::i18n::set_language("ru");
        let now = Local::now();
        let totals = Totals { input: 1200, output: 3400, cache_read: 1_500_000, replies: 3, ..Default::default() };
        let limits = Limits {
            session: Some(Limit { pct: 92.0, resets_at: Some(now + chrono::Duration::minutes(90)) }),
            week: Some(Limit { pct: 40.0, resets_at: Some(now + chrono::Duration::days(3)) }),
        };
        let frames: Vec<Frame> = (0..STYLES.len() as i64).map(|s| frame(s, &View::new(&[], &totals, &limits, now))).collect();
        for (i, a) in frames.iter().enumerate() {
            for b in &frames[i + 1..] {
                assert_ne!(a, b);
            }
        }
        // no limits: the screens still draw (dashes instead of numbers)
        let empty = Limits::default();
        for s in 0..STYLES.len() as i64 {
            let _ = frame(s, &View::new(&[], &totals, &empty, now));
        }
        // out of range falls back to the first style
        assert_eq!(frame(99, &View::new(&[], &totals, &limits, now)), frames[0]);
        // running subagents show up in every style
        let agent = |id: &str| crate::claude::Agent { id: id.into(), kind: String::new(), started: now, seen: now };
        let session = |agents| Session {
            id: "s".into(),
            cwd: "/home/x/divoom".into(),
            state: ClaudeState::Working,
            last_event: String::new(),
            message: String::new(),
            updated: now,
            agents,
        };
        let (idle, busy) = ([session(vec![])], [session(vec![agent("a"), agent("b")])]);
        assert_eq!(View::new(&busy, &totals, &limits, now).agents, 2);
        for s in 0..STYLES.len() as i64 {
            assert_ne!(frame(s, &View::new(&idle, &totals, &limits, now)), frame(s, &View::new(&busy, &totals, &limits, now)), "style {s}");
        }
    }
}
