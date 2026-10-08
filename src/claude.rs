//! Claude Code status (§11): hook events → per-session state, aggregation, timeouts, the alert
//! caption, and installing the hooks into `~/.claude/settings.json`.

use chrono::{DateTime, Local};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const WORKING_TIMEOUT_SECS: i64 = 15 * 60;
pub const SESSION_TIMEOUT_SECS: i64 = 3 * 60 * 60;
pub const HOOK_EVENTS: [&str; 7] =
    ["SessionStart", "SessionEnd", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Notification", "Stop"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum ClaudeState {
    #[default]
    Chilling,
    Working,
    Alerting,
}

impl ClaudeState {
    pub const ALL: [ClaudeState; 3] = [ClaudeState::Working, ClaudeState::Alerting, ClaudeState::Chilling];

    pub fn id(self) -> &'static str {
        match self {
            ClaudeState::Working => "working",
            ClaudeState::Alerting => "alerting",
            ClaudeState::Chilling => "chilling",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "working" => Some(ClaudeState::Working),
            "alerting" => Some(ClaudeState::Alerting),
            "chilling" => Some(ClaudeState::Chilling),
            _ => None,
        }
    }

    /// «Работает / Ждёт вас / Отдыхает»
    pub fn title(self) -> &'static str {
        match self {
            ClaudeState::Working => "Работает",
            ClaudeState::Alerting => "Ждёт вас",
            ClaudeState::Chilling => "Отдыхает",
        }
    }

    /// lowercase form used on device screens: «работает / ждёт вас / отдыхает»
    pub fn lower(self) -> &'static str {
        match self {
            ClaudeState::Working => "работает",
            ClaudeState::Alerting => "ждёт вас",
            ClaudeState::Chilling => "отдыхает",
        }
    }

    pub fn severity(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: String,
    pub cwd: String,
    pub state: ClaudeState,
    pub last_event: String,
    pub message: String,
    pub updated: DateTime<Local>,
}

impl Session {
    /// Project = the last folder of `cwd`; no cwd → the first 8 characters of the id;
    /// the manual check → «проверка».
    pub fn project(&self) -> String {
        if self.id == "manual" {
            return "проверка".to_string();
        }
        if self.cwd.is_empty() {
            return self.id.chars().take(8).collect();
        }
        Path::new(self.cwd.trim_end_matches('/'))
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.cwd.clone())
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "project": self.project(),
            "cwd": self.cwd,
            "state": self.state.id(),
            "event": self.last_event,
            "message": self.message,
            "updated": self.updated.format("%Y-%m-%dT%H:%M:%S").to_string(),
        })
    }
}

#[derive(Debug, Default)]
pub struct ClaudeMonitor {
    sessions: HashMap<String, Session>,
    pub idle_alerts: bool,
}

impl ClaudeMonitor {
    pub fn new(idle_alerts: bool) -> Self {
        ClaudeMonitor { sessions: HashMap::new(), idle_alerts }
    }

    /// Sessions, most recently updated first.
    pub fn sessions(&self) -> Vec<Session> {
        let mut v: Vec<Session> = self.sessions.values().cloned().collect();
        v.sort_by(|a, b| b.updated.cmp(&a.updated).then(a.id.cmp(&b.id)));
        v
    }

    pub fn state(&self) -> ClaudeState {
        self.sessions.values().map(|s| s.state).max().unwrap_or(ClaudeState::Chilling)
    }

    /// Handles one hook body. Returns true if sessions changed.
    pub fn handle_hook(&mut self, hook: &Value, now: DateTime<Local>) -> bool {
        let s = |k: &str| hook.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let event = s("hook_event_name");
        let id = hook.get("session_id").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let cwd = s("cwd");
        let mut message = s("message");
        let tool = s("tool_name");
        if message.is_empty() && event == "PermissionRequest" && !tool.is_empty() {
            message = format!("Claude needs your permission to use {tool}");
        }
        let state = match event.as_str() {
            "SessionEnd" => return self.sessions.remove(&id).is_some(),
            "SessionStart" | "Stop" => ClaudeState::Chilling,
            "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "PreCompact" | "SubagentStart" => ClaudeState::Working,
            "Notification" | "PermissionRequest" => {
                let idle = s("notification_type") == "idle_prompt"
                    || message.to_lowercase().contains("waiting for your input");
                if idle && !self.idle_alerts {
                    // "Claude is waiting for your input" after a finished turn is not an alarm
                    self.sessions.get(&id).map(|s| s.state).unwrap_or(ClaudeState::Chilling)
                } else {
                    ClaudeState::Alerting
                }
            }
            _ => return false, // SubagentStop and anything unknown: keep the current state
        };
        self.set_session_state(&id, &cwd, state, &event, &message, now);
        true
    }

    pub fn set_session_state(&mut self, id: &str, cwd: &str, state: ClaudeState, event: &str, message: &str, now: DateTime<Local>) {
        let s = self.sessions.entry(id.to_string()).or_insert_with(|| Session {
            id: id.to_string(),
            cwd: String::new(),
            state,
            last_event: String::new(),
            message: String::new(),
            updated: now,
        });
        if !cwd.is_empty() {
            s.cwd = cwd.to_string();
        }
        s.state = state;
        s.last_event = event.to_string();
        if !message.is_empty() || state != ClaudeState::Alerting {
            s.message = message.to_string();
        }
        s.updated = now;
    }

    /// The manual check session (`manual`).
    pub fn force_state(&mut self, state: ClaudeState, now: DateTime<Local>) {
        let message = if state == ClaudeState::Alerting { "Claude needs your permission to use Bash" } else { "" };
        self.set_session_state("manual", "", state, "manual", message, now);
    }

    pub fn end_forced_state(&mut self) -> bool {
        self.sessions.remove("manual").is_some()
    }

    pub fn clear(&mut self) -> bool {
        let had = !self.sessions.is_empty();
        self.sessions.clear();
        had
    }

    /// Every 10 s: stuck `working` → `chilling` after 15 min, sessions older than 3 h removed.
    pub fn expire(&mut self, now: DateTime<Local>) -> bool {
        let mut changed = false;
        self.sessions.retain(|_, s| {
            let age = (now - s.updated).num_seconds();
            if age > SESSION_TIMEOUT_SECS {
                changed = true;
                return false;
            }
            if s.state == ClaudeState::Working && age > WORKING_TIMEOUT_SECS {
                s.state = ClaudeState::Chilling;
                changed = true;
            }
            true
        });
        changed
    }

    /// The two caption lines for the alerting scene (§11.5), not yet uppercased.
    pub fn alert_caption(&self) -> Option<(String, String)> {
        let alerting: Vec<Session> =
            self.sessions().into_iter().filter(|s| s.state == ClaudeState::Alerting).collect();
        let first = alerting.first()?;
        let mut line1 = first.project();
        if alerting.len() > 1 {
            line1 += &format!(" +{}", alerting.len() - 1);
        }
        Some((line1, short_question(&first.message)))
    }

    pub fn status_json(&self) -> Value {
        json!({
            "state": self.state().id(),
            "sessions": self.sessions().iter().map(|s| s.to_json()).collect::<Vec<_>>(),
        })
    }
}

/// «… needs your permission to use X» → «Разрешить X?» and the other short forms of §11.5.
pub fn short_question(message: &str) -> String {
    let simplified = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = simplified.to_lowercase();
    const MARK: &str = "needs your permission to use ";
    if let Some(pos) = lower.find(MARK) {
        let tool = simplified[pos + MARK.len()..].trim().trim_end_matches(['.', '?', '!']);
        return format!("Разрешить {tool}?");
    }
    if lower.contains("waiting for your input") {
        return "Ждёт ответа".to_string();
    }
    if lower.contains("needs your attention") {
        return "Нужно ваше внимание".to_string();
    }
    if simplified.is_empty() {
        return "Ждёт вас".to_string();
    }
    simplified
}

// ---------------------------------------------------------------------- hooks in settings.json

pub fn claude_settings_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".claude").join("settings.json")
}

pub fn hook_command(port: u16) -> String {
    format!(
        "curl -s -m 2 -X POST -H 'Content-Type: application/json' --data-binary @- http://127.0.0.1:{port}/hook >/dev/null 2>&1 || true"
    )
}

/// The snippet shown in the UI and copied by «Скопировать JSON».
pub fn hooks_snippet(port: u16) -> String {
    let group = json!([{ "hooks": [{ "type": "command", "command": hook_command(port) }] }]);
    let mut hooks = serde_json::Map::new();
    for e in HOOK_EVENTS {
        hooks.insert(e.to_string(), group.clone());
    }
    serde_json::to_string_pretty(&json!({ "hooks": hooks })).unwrap_or_default()
}

fn is_own_command(cmd: &str) -> bool {
    cmd.contains("curl") && cmd.contains("127.0.0.1:") && cmd.contains("/hook")
}

fn is_own_command_port(cmd: &str, port: u16) -> bool {
    is_own_command(cmd) && cmd.contains(&format!("127.0.0.1:{port}/hook"))
}

/// Removes our hook entries (any port), empty groups and empty events.
fn strip_own(root: &mut Value) {
    let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) else { return };
    for groups in hooks.values_mut() {
        if let Some(arr) = groups.as_array_mut() {
            for g in arr.iter_mut() {
                if let Some(list) = g.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                    list.retain(|h| !h.get("command").and_then(|c| c.as_str()).map(is_own_command).unwrap_or(false));
                }
            }
            arr.retain(|g| g.get("hooks").and_then(|h| h.as_array()).map(|l| !l.is_empty()).unwrap_or(true));
        }
    }
    hooks.retain(|_, v| v.as_array().map(|a| !a.is_empty()).unwrap_or(true));
    if hooks.is_empty() {
        if let Some(o) = root.as_object_mut() {
            o.remove("hooks");
        }
    }
}

fn read_settings(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.is_object() => Ok(v),
            _ => Err(format!("{} содержит некорректный JSON — хуки не установлены", path.display())),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// `settings.json.minitoo-backup-yyyyMMdd-HHmmss`
pub fn backup(path: &Path) -> Result<Option<PathBuf>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let b = path.with_file_name(format!(
        "{}.minitoo-backup-{stamp}",
        path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()
    ));
    std::fs::copy(path, &b).map_err(|_| "Не удалось сделать резервную копию".to_string())?;
    Ok(Some(b))
}

fn write_settings(path: &Path, v: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())? + "\n";
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn install_hooks_at(path: &Path, port: u16) -> Result<String, String> {
    let mut root = read_settings(path)?;
    let b = backup(path)?;
    strip_own(&mut root);
    let obj = root.as_object_mut().ok_or("settings.json")?;
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let hooks = hooks.as_object_mut().unwrap();
    for e in HOOK_EVENTS {
        let groups = hooks.entry(e.to_string()).or_insert_with(|| json!([]));
        if !groups.is_array() {
            *groups = json!([]);
        }
        groups
            .as_array_mut()
            .unwrap()
            .push(json!({ "hooks": [{ "type": "command", "command": hook_command(port) }] }));
    }
    write_settings(path, &root)?;
    let _ = b;
    Ok("Хуки установлены. Новые сессии Claude Code начнут присылать статус.".to_string())
}

pub fn uninstall_hooks_at(path: &Path) -> Result<String, String> {
    if !path.exists() {
        return Err(format!("Файл {} не найден", path.display()));
    }
    let mut root = read_settings(path)?;
    let b = backup(path)?;
    strip_own(&mut root);
    write_settings(path, &root)?;
    let _ = b;
    Ok("Хуки удалены.".to_string())
}

/// «Установлены» = every one of the 7 events has our entry with the current port.
pub fn hooks_installed_at(path: &Path, port: u16) -> bool {
    let Ok(root) = read_settings(path) else { return false };
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else { return false };
    HOOK_EVENTS.iter().all(|e| {
        hooks.get(*e).and_then(|g| g.as_array()).is_some_and(|groups| {
            groups.iter().any(|g| {
                g.get("hooks").and_then(|h| h.as_array()).is_some_and(|l| {
                    l.iter().any(|h| h.get("command").and_then(|c| c.as_str()).is_some_and(|c| is_own_command_port(c, port)))
                })
            })
        })
    })
}

pub fn install_hooks(port: u16) -> Result<String, String> {
    install_hooks_at(&claude_settings_path(), port)
}
pub fn uninstall_hooks() -> Result<String, String> {
    uninstall_hooks_at(&claude_settings_path())
}
pub fn hooks_installed(port: u16) -> bool {
    hooks_installed_at(&claude_settings_path(), port)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(event: &str, id: &str, extra: Value) -> Value {
        let mut v = json!({"hook_event_name": event, "session_id": id, "cwd": "/home/u/divoom"});
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, val) in e {
                o.insert(k.clone(), val.clone());
            }
        }
        v
    }

    #[test]
    fn aggregation_and_events() {
        let now = Local::now();
        let mut m = ClaudeMonitor::new(false);
        assert_eq!(m.state(), ClaudeState::Chilling);
        m.handle_hook(&hook("SessionStart", "a", json!({})), now);
        m.handle_hook(&hook("PreToolUse", "b", json!({})), now);
        assert_eq!(m.state(), ClaudeState::Working);
        m.handle_hook(&hook("PermissionRequest", "a", json!({"tool_name": "Bash"})), now);
        assert_eq!(m.state(), ClaudeState::Alerting);
        let (l1, l2) = m.alert_caption().unwrap();
        assert_eq!(l1, "divoom");
        assert_eq!(l2, "Разрешить Bash?");
        // alert without text keeps the previous question
        m.handle_hook(&hook("Notification", "a", json!({})), now);
        assert_eq!(m.alert_caption().unwrap().1, "Разрешить Bash?");
        // idle prompt is not an alarm by default
        m.handle_hook(&hook("Notification", "c", json!({"notification_type": "idle_prompt"})), now);
        assert_eq!(m.sessions().iter().find(|s| s.id == "c").unwrap().state, ClaudeState::Chilling);
        m.handle_hook(&hook("SubagentStop", "b", json!({})), now);
        assert_eq!(m.sessions().iter().find(|s| s.id == "b").unwrap().state, ClaudeState::Working);
        m.handle_hook(&hook("SessionEnd", "a", json!({})), now);
        assert_eq!(m.state(), ClaudeState::Working);
    }

    #[test]
    fn idle_alerts_option() {
        let mut m = ClaudeMonitor::new(true);
        m.handle_hook(&hook("Notification", "x", json!({"message": "Claude is waiting for your input"})), Local::now());
        assert_eq!(m.state(), ClaudeState::Alerting);
        assert_eq!(m.alert_caption().unwrap().1, "Ждёт ответа");
    }

    #[test]
    fn timeouts() {
        let now = Local::now();
        let mut m = ClaudeMonitor::new(false);
        m.handle_hook(&hook("PreToolUse", "w", json!({})), now - chrono::Duration::minutes(16));
        m.handle_hook(&hook("Stop", "old", json!({})), now - chrono::Duration::hours(4));
        assert!(m.expire(now));
        assert_eq!(m.sessions().len(), 1);
        assert_eq!(m.state(), ClaudeState::Chilling);
    }

    #[test]
    fn captions() {
        assert_eq!(short_question(""), "Ждёт вас");
        assert_eq!(short_question("Claude  needs your attention"), "Нужно ваше внимание");
        assert_eq!(short_question("Build   finished"), "Build finished");
        let now = Local::now();
        let mut m = ClaudeMonitor::new(false);
        m.force_state(ClaudeState::Alerting, now);
        m.handle_hook(&hook("Notification", "z", json!({"message": "Claude needs your permission to use Edit"})), now - chrono::Duration::seconds(5));
        let (l1, l2) = m.alert_caption().unwrap();
        assert_eq!(l1, "проверка +1");
        assert_eq!(l2, "Разрешить Bash?");
    }

    #[test]
    fn install_and_remove_hooks() {
        let dir = std::env::temp_dir().join(format!("minitoo-hooks-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"notify-send done"}]},
               {"hooks":[{"type":"command","command":"curl -s http://127.0.0.1:9999/hook"}]}]}}"#,
        )
        .unwrap();
        assert!(!hooks_installed_at(&path, 47800));
        install_hooks_at(&path, 47800).unwrap();
        assert!(hooks_installed_at(&path, 47800));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["model"], "opus");
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "foreign hook kept, old own one replaced");
        uninstall_hooks_at(&path).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(v["hooks"].get("PreToolUse").is_none());
        std::fs::write(&path, "{broken").unwrap();
        assert!(install_hooks_at(&path, 47800).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
