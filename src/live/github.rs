//! GitHub Actions (`github`, §8.7): the latest push run of up to 4 repositories.

use super::{GithubView, LiveMode, ModeCommand, ModeCx, ModeMsg, ModeView, RepoView, RunState};
use crate::canvas::{r, Align, Canvas, R};
use crate::color::Color;
use crate::fonts::FontSpec;
use crate::frame::Frame;
use std::collections::HashMap;
use std::time::Duration;

pub const MAX_REPOS: usize = 4;
// catalog keys of the add errors; `tr!` them for display
pub const ERR_FORMAT: &str = "github.err.format";
pub const ERR_DUPLICATE: &str = "github.err.duplicate";
pub const ERR_FULL: &str = "github.err.full";

/// The latest run of a repository as far as we know.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    /// "" loading, "none" no runs, else the API `status` (queued / in_progress / completed …)
    pub status: String,
    pub conclusion: String,
    pub branch: String,
    pub workflow: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub updated: String,
    pub etag: String,
    pub error: String,
}

impl Run {
    fn state(&self) -> RunState {
        if !self.error.is_empty() {
            RunState::Error
        } else if self.status == "none" {
            RunState::Neutral
        } else if self.status == "completed" {
            match self.conclusion.as_str() {
                "success" => RunState::Passed,
                "cancelled" | "skipped" => RunState::Neutral,
                _ => RunState::Failed,
            }
        } else if !self.status.is_empty() {
            RunState::Running
        } else {
            RunState::Loading
        }
    }

    /// Text under the repository name in the UI: «успешно  ·  CI #123 (main)».
    fn detail(&self) -> String {
        let base = match self.state() {
            RunState::Error => return self.error.clone(),
            RunState::Passed => tr!("github.state.passed").to_string(),
            RunState::Failed => tr!("github.state.failed").to_string(),
            RunState::Running => tr!("github.state.running").to_string(),
            RunState::Loading => tr!("github.state.loading").to_string(),
            RunState::Neutral if self.status == "none" => tr!("github.no_runs").to_string(),
            RunState::Neutral => self.conclusion.clone(),
        };
        if self.number > 0 { format!("{base}  ·  {} #{} ({})", self.workflow, self.number, self.branch) } else { base }
    }
}

/// `owner/repo` from user input: a plain name or a link; `https://github.com/`, `.git` and
/// trailing slashes are removed. `None` when it does not look like a repository.
pub fn normalize_repo(input: &str) -> Option<String> {
    let mut s = input.trim();
    for prefix in ["https://", "http://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
            break;
        }
    }
    let had_scheme = s.len() != input.trim().len();
    if let Some(rest) = s.strip_prefix("www.") {
        if rest.starts_with("github.com/") {
            s = rest;
        }
    }
    match s.strip_prefix("github.com/") {
        Some(rest) => s = rest,
        None if had_scheme => return None,
        None => {}
    }
    let s = s.trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s).trim_end_matches('/');
    let mut parts = s.split('/');
    let owner = parts.next().unwrap_or("");
    let repo = parts.next().unwrap_or("");
    if owner.is_empty() || repo.is_empty() || owner.contains(char::is_whitespace) || repo.contains(char::is_whitespace) {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// Adds a repository to `repos`, or returns the catalog key of one of the three errors.
pub fn add_repo(repos: &mut Vec<String>, input: &str) -> Result<String, &'static str> {
    let repo = normalize_repo(input).ok_or(ERR_FORMAT)?;
    if repos.iter().any(|r| r.eq_ignore_ascii_case(&repo)) {
        return Err(ERR_DUPLICATE);
    }
    if repos.len() >= MAX_REPOS {
        return Err(ERR_FULL);
    }
    repos.push(repo.clone());
    Ok(repo)
}

/// HTTP status → error text (0 = no network).
pub fn error_text(code: u16) -> String {
    match code {
        404 => tr!("github.err.not_found").to_string(),
        403 | 429 => tr!("github.err.rate_limit").to_string(),
        0 => tr!("github.err.offline").to_string(),
        c => tr!("github.err.http", code = c),
    }
}

struct Reply {
    repo: String,
    code: u16,
    etag: String,
    /// the latest run, or `None` if there are no runs (only for 200)
    run: Option<Run>,
}

pub struct Github {
    loaded: bool,
    repos: Vec<String>,
    token: String,
    runs: HashMap<String, Run>,
    add_error: Option<String>,
    running: bool,
    /// periodic fetch timers carry this generation; changing the interval restarts them
    generation: u64,
}

impl Default for Github {
    fn default() -> Self {
        Self::new()
    }
}

impl Github {
    pub fn new() -> Self {
        Github {
            loaded: false,
            repos: Vec::new(),
            token: String::new(),
            runs: HashMap::new(),
            add_error: None,
            running: false,
            generation: 0,
        }
    }

    fn ensure_loaded(&mut self, cx: &ModeCx) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        self.repos = cx.settings.list("github/repos").into_iter().filter_map(|r| normalize_repo(&r)).take(MAX_REPOS).collect();
        self.token = cx.settings.string("github/token", "").trim().to_string();
    }

    /// Repositories and runs for offscreen rendering.
    pub fn set_state(&mut self, repos: Vec<String>, runs: HashMap<String, Run>) {
        self.loaded = true;
        self.repos = repos;
        self.runs = runs;
    }

    /// Polling interval, seconds: 60 with a token, otherwise `max(120, repos·90)`.
    pub fn interval(&self) -> u64 {
        if !self.token.is_empty() {
            return 60;
        }
        (self.repos.len() as u64 * 90).max(120)
    }

    fn restart_timer(&mut self, cx: &mut ModeCx) {
        self.generation += 1;
        if self.running {
            cx.timer(Duration::from_secs(self.interval()), self.generation);
        }
    }

    fn save_repos(&mut self, cx: &mut ModeCx) {
        cx.settings.set_list("github/repos", &self.repos);
        self.restart_timer(cx);
        if self.running {
            self.fetch(cx);
        } else {
            self.render(cx);
        }
    }

    fn fetch(&mut self, cx: &mut ModeCx) {
        if self.repos.is_empty() {
            cx.set_status(tr!("github.status.empty"));
            self.render(cx);
            return;
        }
        for repo in self.repos.clone() {
            let http = cx.http().clone();
            let token = self.token.clone();
            let etag = self.runs.get(&repo).map(|r| r.etag.clone()).unwrap_or_default();
            cx.spawn(async move { Box::new(request(http, repo, token, etag).await) as ModeMsg });
        }
    }

    pub fn frame(&self) -> Frame {
        self.frame_and_failing().0
    }

    fn frame_and_failing(&self) -> (Frame, usize) {
        let mut c = Canvas::device();
        c.fill(Color::rgb(13, 17, 23));
        c.text(r(8.0, 2.0, 144.0, 18.0), Align::LEFT, "GitHub Actions", FontSpec::bold(11.0), Color::WHITE);
        if self.repos.is_empty() {
            // two centred lines in (8, 40, 144×40), 12 px apart like Qt's line spacing
            for (i, line) in [tr!("github.screen.empty1"), tr!("github.screen.empty2")].iter().enumerate() {
                let y = 48.5 + 12.0 * i as f32;
                c.text(r(8.0, y, 144.0, 12.0), Align::CENTER, line, FontSpec::sans(10.0), Color::rgb(140, 150, 170));
            }
        }
        let mut failing = 0;
        let mut y = 24.0;
        for repo in &self.repos {
            let run = self.runs.get(repo).cloned().unwrap_or_default();
            let mut color = Color::rgb(140, 150, 170);
            let (mark, text): (&str, String) = match run.state() {
                RunState::Error => ("?", run.error.clone()),
                RunState::Loading => ("…", tr!("github.row.loading").into()),
                RunState::Neutral if run.status == "none" => ("–", tr!("github.no_runs").into()),
                RunState::Neutral => ("–", run.conclusion.clone()),
                RunState::Passed => {
                    color = Color::rgb(60, 190, 100);
                    ("✓", tr!("github.row.passed").into())
                }
                RunState::Failed => {
                    color = Color::rgb(230, 70, 60);
                    failing += 1;
                    ("✗", tr!("github.row.failed").into())
                }
                RunState::Running => {
                    color = Color::rgb(230, 180, 50);
                    ("●", tr!("github.row.running").into())
                }
            };
            c.fill_round_rect(6.0, y, 148.0, 22.0, 4.0, Color::rgb(24, 30, 40));
            text_clipped(&mut c, r(11.0, y, 14.0, 22.0), Align::CENTER, mark, FontSpec::bold(12.0), color);
            let name = repo.split_once('/').map(|(_, n)| n).unwrap_or(repo);
            let name_font = FontSpec::bold(10.0);
            text_clipped(&mut c, r(28.0, y, 84.0, 22.0), Align::LEFT, &Canvas::elide(name, name_font, 84.0), name_font, Color::WHITE);
            text_clipped(&mut c, r(100.0, y, 50.0, 22.0), Align::RIGHT, &text, FontSpec::sans(9.0), color);
            y += 25.0;
        }
        (c.to_frame(), failing)
    }

    fn status(&self, failing: usize) -> String {
        if failing > 0 {
            return tr!("github.status.failing", failing = failing, total = self.repos.len());
        }
        if self.interval() >= 120 {
            trn!("github.status.every_minutes", self.repos.len(), min = self.interval() / 60)
        } else {
            trn!("github.status.every_minute", self.repos.len())
        }
    }
}

/// `QPainter::drawText(rect, …)` clips to the rectangle: the text is drawn on a layer of the
/// rectangle's size and composed.
fn text_clipped(c: &mut Canvas, rect: R, align: Align, s: &str, font: FontSpec, color: Color) {
    let (x0, y0) = (rect.x.floor(), rect.y.floor());
    let w = (rect.right().ceil() - x0).max(1.0) as u32;
    let h = (rect.bottom().ceil() - y0).max(1.0) as u32;
    let mut layer = Canvas::new(w, h);
    layer.text(r(rect.x - x0, rect.y - y0, rect.w, rect.h), align, s, font, color);
    c.draw_canvas(&layer, x0, y0, 1.0);
}

async fn request(http: reqwest::Client, repo: String, token: String, etag: String) -> Reply {
    let url = format!("https://api.github.com/repos/{repo}/actions/runs?per_page=1&event=push");
    let mut req = http
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "minitoo-studio")
        .timeout(Duration::from_secs(30));
    if !token.is_empty() {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    // a 304 for an unchanged ETag does not count against the rate limit
    if !etag.is_empty() {
        req = req.header("If-None-Match", etag);
    }
    let Ok(resp) = req.send().await else { return Reply { repo, code: 0, etag: String::new(), run: None } };
    let code = resp.status().as_u16();
    let etag = resp.headers().get("etag").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if code != 200 {
        return Reply { repo, code, etag, run: None };
    }
    let run = match resp.json::<serde_json::Value>().await {
        Ok(v) => parse_runs(&v),
        Err(_) => return Reply { repo, code: 0, etag: String::new(), run: None },
    };
    Reply { repo, code, etag, run }
}

/// The first run of a `/actions/runs` reply (`None` = no runs).
pub fn parse_runs(v: &serde_json::Value) -> Option<Run> {
    let o = v.get("workflow_runs")?.as_array()?.first()?;
    let s = |k: &str| o.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    Some(Run {
        status: s("status"),
        conclusion: s("conclusion"),
        branch: s("head_branch"),
        workflow: s("name"),
        number: o.get("run_number").and_then(|x| x.as_i64()).unwrap_or(0),
        title: s("display_title"),
        url: s("html_url"),
        updated: s("updated_at"),
        ..Default::default()
    })
}

impl LiveMode for Github {
    fn id(&self) -> &'static str {
        "github"
    }
    fn title(&self) -> &'static str {
        "GitHub Actions"
    }
    fn subtitle(&self) -> &'static str {
        tr!("github.subtitle")
    }
    fn icon(&self) -> &'static str {
        "branch"
    }

    fn start(&mut self, cx: &mut ModeCx) {
        self.ensure_loaded(cx);
        self.running = true;
        self.fetch(cx);
        self.restart_timer(cx);
    }

    fn stop(&mut self, _cx: &mut ModeCx) {
        self.running = false;
    }

    fn render(&mut self, cx: &mut ModeCx) {
        self.ensure_loaded(cx);
        let (frame, failing) = self.frame_and_failing();
        if !self.repos.is_empty() {
            cx.set_status(self.status(failing));
        }
        cx.publish(frame);
    }

    fn on_timer(&mut self, cx: &mut ModeCx, token: u64) {
        if token == self.generation && self.running {
            self.fetch(cx);
            cx.timer(Duration::from_secs(self.interval()), self.generation);
        }
    }

    fn on_message(&mut self, cx: &mut ModeCx, msg: ModeMsg) {
        let Ok(reply) = msg.downcast::<Reply>() else { return };
        let Reply { repo, code, etag, run } = *reply;
        if !self.repos.contains(&repo) || code == 304 {
            return; // removed meanwhile / unchanged
        }
        let entry = self.runs.entry(repo).or_default();
        if code != 200 {
            entry.error = error_text(code);
        } else {
            let mut new = run.unwrap_or_else(|| Run { status: "none".into(), ..Default::default() });
            new.etag = etag;
            *entry = new;
        }
        self.render(cx);
    }

    fn command(&mut self, cx: &mut ModeCx, cmd: ModeCommand) {
        self.ensure_loaded(cx);
        match cmd {
            ModeCommand::GithubAdd(text) => match add_repo(&mut self.repos, &text) {
                Ok(_) => {
                    self.add_error = None;
                    self.save_repos(cx);
                }
                Err(key) => self.add_error = Some(tr!(key).to_string()),
            },
            ModeCommand::GithubRemove(repo) => {
                let before = self.repos.len();
                self.repos.retain(|r| *r != repo);
                if self.repos.len() != before {
                    self.runs.remove(&repo);
                    self.save_repos(cx);
                }
            }
            ModeCommand::GithubToken(t) => {
                self.token = t.trim().to_string();
                cx.settings.set_string("github/token", &self.token);
                self.restart_timer(cx);
                if self.running {
                    self.fetch(cx);
                }
            }
            ModeCommand::GithubClearToken => {
                self.token.clear();
                cx.settings.remove("github/token");
                self.restart_timer(cx);
                if self.running {
                    self.fetch(cx);
                }
            }
            _ => {}
        }
    }

    fn view(&self) -> ModeView {
        ModeView::Github(GithubView {
            repos: self
                .repos
                .iter()
                .map(|repo| {
                    let run = self.runs.get(repo).cloned().unwrap_or_default();
                    RepoView {
                        name: repo.clone(),
                        state: run.state(),
                        detail: run.detail(),
                        url: if run.url.is_empty() { format!("https://github.com/{repo}/actions") } else { run.url.clone() },
                    }
                })
                .collect(),
            has_token: !self.token.is_empty(),
            add_error: self.add_error.clone(),
            interval: self.interval(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_input() {
        assert_eq!(normalize_repo("cli/cli").as_deref(), Some("cli/cli"));
        assert_eq!(normalize_repo("  https://github.com/neovim/neovim.git/ ").as_deref(), Some("neovim/neovim"));
        assert_eq!(normalize_repo("github.com/rust-lang/rust/").as_deref(), Some("rust-lang/rust"));
        assert_eq!(normalize_repo("https://www.github.com/a/b/actions/runs/1").as_deref(), Some("a/b"));
        assert_eq!(normalize_repo("http://github.com/a/b///").as_deref(), Some("a/b"));
        assert_eq!(normalize_repo("justname"), None);
        assert_eq!(normalize_repo("/x"), None);
        assert_eq!(normalize_repo("https://gitlab.com/a/b"), None);
        assert_eq!(normalize_repo(""), None);
    }

    #[test]
    fn add_errors() {
        let mut v = Vec::new();
        assert_eq!(add_repo(&mut v, "nope"), Err(ERR_FORMAT));
        assert_eq!(add_repo(&mut v, "cli/cli"), Ok("cli/cli".to_string()));
        assert_eq!(add_repo(&mut v, "https://github.com/CLI/cli"), Err(ERR_DUPLICATE));
        for r in ["a/b", "c/d", "e/f"] {
            add_repo(&mut v, r).unwrap();
        }
        assert_eq!(add_repo(&mut v, "g/h"), Err(ERR_FULL));
        assert_eq!(v.len(), 4);
    }

    #[test]
    fn errors_and_intervals() {
        assert_eq!(error_text(404), "не найден или приватный");
        assert_eq!(error_text(403), "лимит API GitHub");
        assert_eq!(error_text(429), "лимит API GitHub");
        assert_eq!(error_text(0), "нет сети");
        assert_eq!(error_text(500), "ошибка 500");
        let mut g = Github::new();
        g.set_state(vec!["a/b".into()], HashMap::new());
        assert_eq!(g.interval(), 120);
        g.repos = vec!["a/b".into(), "c/d".into(), "e/f".into()];
        assert_eq!(g.interval(), 270);
        assert_eq!(g.status(0), "3 репоз., обновление раз в 4 мин");
        g.token = "t".into();
        assert_eq!(g.interval(), 60);
        assert_eq!(g.status(0), "3 репоз., обновление раз в минуту");
        assert_eq!(g.status(1), "упавших: 1 из 3");
    }

    #[test]
    fn run_parsing_and_states() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"total_count":1,"workflow_runs":[{"status":"completed","conclusion":"success","head_branch":"main","name":"CI","run_number":42,"display_title":"x","html_url":"https://github.com/a/b/actions/runs/1","updated_at":"2026-10-08T10:00:00Z"}]}"#,
        )
        .unwrap();
        let run = parse_runs(&v).unwrap();
        assert_eq!(run.state(), RunState::Passed);
        assert_eq!(run.detail(), "успешно  ·  CI #42 (main)");
        assert!(parse_runs(&serde_json::json!({"workflow_runs": []})).is_none());
        let failed = Run { status: "completed".into(), conclusion: "failure".into(), ..Default::default() };
        assert_eq!(failed.state(), RunState::Failed);
        let running = Run { status: "in_progress".into(), ..Default::default() };
        assert_eq!(running.state(), RunState::Running);
        assert_eq!(Run::default().state(), RunState::Loading);
        let err = Run { error: error_text(404), ..Default::default() };
        assert_eq!((err.state(), err.detail()), (RunState::Error, "не найден или приватный".to_string()));
    }
}
