//! Статистика Claude (`claudestats`, §8.6): today's tokens from Claude Code transcripts
//! (`~/.claude/projects/**/*.jsonl`, read incrementally every 30 s) plus the live sessions.

use super::{LiveMode, ModeCx, ModeMsg};
use crate::canvas::{r, Align, Canvas};
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
const EVERY: Duration = Duration::from_secs(30);

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

fn state_color(s: ClaudeState) -> Color {
    match s {
        ClaudeState::Alerting => Color::rgb(230, 64, 48),
        ClaudeState::Working => Color::ACCENT,
        ClaudeState::Chilling => Color::rgb(90, 120, 190),
    }
}

/// The statistics screen.
pub fn frame(sessions: &[Session], t: &Totals) -> Frame {
    let dim = Color::rgb(150, 145, 165);
    let head = Color::rgb(30, 18, 14);
    let mut c = Canvas::device();
    c.fill(Color::rgb(18, 15, 22));
    c.fill_rect(0.0, 0.0, 160.0, 20.0, Color::ACCENT);
    c.text(r(8.0, 0.0, 120.0, 20.0), Align::LEFT, "Claude Code", FontSpec::bold(12.0), head);
    c.text(r(110.0, 0.0, 42.0, 20.0), Align::RIGHT, &sessions.len().to_string(), FontSpec::bold(12.0), head);

    let mut y = 26.0;
    if sessions.is_empty() {
        c.text(r(8.0, y, 144.0, 18.0), Align::LEFT, tr!("claudestats.no_sessions"), FontSpec::sans(10.0), dim);
    }
    for s in sessions.iter().take(3) {
        let color = state_color(s.state);
        c.fill_circle(12.0, y + 9.0, 4.0, color);
        let name_font = FontSpec::bold(11.0);
        c.text(r(22.0, y, 80.0, 18.0), Align::LEFT, &Canvas::elide(&s.project(), name_font, 78.0), name_font, Color::WHITE);
        c.text(r(96.0, y, 56.0, 18.0), Align::RIGHT, s.state.lower(), FontSpec::sans(10.0), color);
        y += 20.0;
    }
    if sessions.len() > 3 {
        c.text(r(22.0, y - 4.0, 130.0, 10.0), Align::LEFT, &tr!("claudestats.more", count = sessions.len() - 3), FontSpec::sans(8.0), dim);
    }

    c.line(8.0, 90.0, 152.0, 90.0, 1.0, Color::rgb(55, 50, 65));
    c.text(r(8.0, 94.0, 60.0, 16.0), Align::LEFT, tr!("claudestats.today"), FontSpec::sans(9.0), dim);
    c.text(r(50.0, 92.0, 102.0, 18.0), Align::RIGHT, &tr!("claudestats.tokens_short", total = compact(t.all())), FontSpec::bold(13.0), Color::WHITE);
    c.text(r(8.0, 111.0, 80.0, 14.0), Align::LEFT, &tr!("claudestats.replies", count = t.replies), FontSpec::sans(9.0), dim);
    c.text(r(72.0, 111.0, 80.0, 14.0), Align::RIGHT, &tr!("claudestats.output", output = compact(t.output)), FontSpec::sans(9.0), dim);
    c.to_frame()
}

struct ScanMsg(Scanner);

pub struct ClaudeStats {
    /// `None` while a scan runs on the blocking pool
    scanner: Option<Scanner>,
    totals: Totals,
}

impl Default for ClaudeStats {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeStats {
    pub fn new() -> Self {
        ClaudeStats { scanner: Some(Scanner::default()), totals: Totals::default() }
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
        // a scan still running when the mode stopped never came back: start over
        if self.scanner.is_none() {
            self.scanner = Some(Scanner::default());
        }
        self.scan(cx);
        cx.timer(EVERY, SCAN);
    }

    fn render(&mut self, cx: &mut ModeCx) {
        cx.publish(frame(cx.sessions, &self.totals));
    }

    fn on_timer(&mut self, cx: &mut ModeCx, token: u64) {
        if token == SCAN {
            self.scan(cx);
            cx.timer(EVERY, SCAN);
        }
    }

    fn on_message(&mut self, cx: &mut ModeCx, msg: ModeMsg) {
        let Ok(msg) = msg.downcast::<ScanMsg>() else { return };
        let ScanMsg(scanner) = *msg;
        self.totals = scanner.totals;
        self.scanner = Some(scanner);
        cx.set_status(summary(&self.totals));
        self.render(cx);
    }

    fn on_sessions_changed(&mut self, cx: &mut ModeCx) {
        self.render(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
