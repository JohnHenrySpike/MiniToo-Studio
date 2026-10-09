//! Installed applications (freedesktop `.desktop` files), to find the icon of the application
//! that sent a notification (§10).
//!
//! A notification names its sender by the `desktop-entry` hint (the desktop file id) or only by
//! `app_name`, and the icon in the theme often has a third name (`com.anthropic.Claude` →
//! `claude-desktop`). The desktop files are read from `$XDG_DATA_HOME/applications` and every
//! `$XDG_DATA_DIRS/*/applications` (Flatpak exports included) and kept for half a minute.

/// `Icon=` of the desktop file with this id (`org.telegram.desktop`, with or without
/// `.desktop`).
pub fn icon_for_id(id: &str) -> Option<String> {
    let id = id.trim();
    let id = id.strip_suffix(".desktop").unwrap_or(id);
    if id.is_empty() {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        linux::icon_for_id(id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// `Icon=` of the application that calls itself `name`: matched against the desktop file id,
/// `StartupWMClass`, `Name` and the program in `Exec`, ignoring case, spaces and punctuation
/// («Telegram Desktop» → `StartupWMClass=TelegramDesktop`).
pub fn icon_for_app(name: &str) -> Option<String> {
    let key = normalize(name);
    if key.is_empty() {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        linux::icon_for_app(&key)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Lowercase letters and digits only.
fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Entry {
    id: String,
    name: String,
    wm_class: String,
    exec: String,
    icon: String,
}

/// The `[Desktop Entry]` group of a desktop file; `None` for hidden entries and ones without
/// an icon.
fn parse(id: &str, text: &str) -> Option<Entry> {
    let mut e = Entry { id: id.to_string(), ..Default::default() };
    let mut in_main = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if in_main {
                break;
            }
            in_main = t == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((k, v)) = t.split_once('=') else { continue };
        let v = v.trim().to_string();
        match k.trim() {
            "Name" => e.name = v,
            "Icon" => e.icon = v,
            "StartupWMClass" => e.wm_class = v,
            "Exec" => e.exec = program(&v),
            "Hidden" if v == "true" => return None,
            _ => {}
        }
    }
    (!e.icon.is_empty()).then_some(e)
}

/// The program of an `Exec=` line without its directory (`env FOO=1 /opt/x/app %U` → `app`).
fn program(exec: &str) -> String {
    let mut words = exec.split_whitespace().map(|w| w.trim_matches('"'));
    let mut first = words.next().unwrap_or("");
    if first.rsplit('/').next() == Some("env") {
        first = words.find(|w| !w.contains('=') && !w.starts_with('-')).unwrap_or("");
    }
    first.rsplit('/').next().unwrap_or("").to_string()
}

/// How well `key` (normalized) names the entry; lower is better.
fn score(e: &Entry, key: &str) -> Option<u8> {
    if normalize(&e.id) == key {
        Some(0)
    } else if normalize(&e.wm_class) == key {
        Some(1)
    } else if normalize(&e.name) == key {
        Some(2)
    } else if e.id.split(['.', '-', '_']).any(|part| part.len() > 2 && normalize(part) == key) {
        Some(3)
    } else if normalize(&e.exec) == key {
        Some(4)
    } else {
        None
    }
}

fn best_match<'a>(entries: &'a [Entry], key: &str) -> Option<&'a Entry> {
    entries.iter().filter_map(|e| score(e, key).map(|s| (s, e))).min_by_key(|(s, _)| *s).map(|(_, e)| e)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{Entry, best_match, parse};
    use parking_lot::Mutex;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const KEEP: Duration = Duration::from_secs(30);

    pub fn icon_for_id(id: &str) -> Option<String> {
        let all = entries();
        all.iter().find(|e| e.id == id).or_else(|| all.iter().find(|e| e.id.eq_ignore_ascii_case(id))).map(|e| e.icon.clone())
    }

    pub fn icon_for_app(key: &str) -> Option<String> {
        best_match(&entries(), key).map(|e| e.icon.clone())
    }

    fn entries() -> Arc<Vec<Entry>> {
        static CACHE: Mutex<Option<(Instant, Arc<Vec<Entry>>)>> = Mutex::new(None);
        let mut cache = CACHE.lock();
        if let Some((at, all)) = cache.as_ref()
            && at.elapsed() < KEEP
        {
            return all.clone();
        }
        let all = Arc::new(scan());
        *cache = Some((Instant::now(), all.clone()));
        all
    }

    fn app_dirs() -> Vec<PathBuf> {
        let mut v = Vec::new();
        match std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()) {
            Some(d) => v.push(d.join("applications")),
            None => v.extend(dirs::home_dir().map(|h| h.join(".local/share/applications"))),
        }
        let xdg = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
        let xdg = if xdg.trim().is_empty() { "/usr/local/share:/usr/share".to_string() } else { xdg };
        v.extend(xdg.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("applications")));
        let mut seen = std::collections::HashSet::new();
        v.retain(|p| seen.insert(p.clone()));
        v
    }

    /// Every desktop file; an id found in an earlier directory hides later ones.
    fn scan() -> Vec<Entry> {
        let mut out: Vec<Entry> = Vec::new();
        let mut ids = std::collections::HashSet::new();
        for dir in app_dirs() {
            let mut files = Vec::new();
            collect(&dir, &dir, 0, &mut files);
            for (id, path) in files {
                if !ids.insert(id.clone()) {
                    continue;
                }
                if let Some(e) = std::fs::read_to_string(&path).ok().and_then(|t| parse(&id, &t)) {
                    out.push(e);
                }
            }
        }
        out
    }

    /// Desktop files under `dir`; the id of `dir/kde/x.desktop` is `kde-x`.
    fn collect(root: &Path, dir: &Path, depth: u32, out: &mut Vec<(String, PathBuf)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for item in rd.flatten() {
            let path = item.path();
            if path.is_dir() {
                if depth < 3 {
                    collect(root, &path, depth + 1, out);
                }
                continue;
            }
            let Some(rel) = path.strip_prefix(root).ok().and_then(|r| r.to_str()) else { continue };
            if let Some(stem) = rel.strip_suffix(".desktop") {
                out.push((stem.replace('/', "-"), path.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = "[Desktop Entry]\nName=Claude\nExec=/usr/bin/claude-desktop %u\nIcon=claude-desktop\n\
                          StartupWMClass=com.anthropic.Claude\n\n[Desktop Action new]\nName=New Chat\nIcon=other\n";
    const TELEGRAM: &str = "[Desktop Entry]\nName=Telegram\nExec=env QT_QPA=x Telegram -- %u\nIcon=org.telegram.desktop\n\
                            StartupWMClass=TelegramDesktop\n";

    #[test]
    fn parses_main_group() {
        let e = parse("com.anthropic.Claude", CLAUDE).unwrap();
        assert_eq!((e.name.as_str(), e.icon.as_str(), e.exec.as_str()), ("Claude", "claude-desktop", "claude-desktop"));
        assert_eq!(parse("t", TELEGRAM).unwrap().exec, "Telegram");
        assert!(parse("x", "[Desktop Entry]\nName=X\nIcon=x\nHidden=true\n").is_none());
        assert!(parse("x", "[Desktop Entry]\nName=X\n").is_none());
    }

    #[test]
    fn matches_app_names() {
        let all = vec![
            parse("com.anthropic.Claude", CLAUDE).unwrap(),
            parse("org.telegram.desktop", TELEGRAM).unwrap(),
            parse("firefox", "[Desktop Entry]\nName=Firefox\nExec=/usr/lib/firefox/firefox %u\nIcon=firefox\n").unwrap(),
        ];
        let icon = |name: &str| best_match(&all, &normalize(name)).map(|e| e.icon.as_str());
        assert_eq!(icon("Claude"), Some("claude-desktop"));
        assert_eq!(icon("Telegram Desktop"), Some("org.telegram.desktop"));
        assert_eq!(icon("telegram"), Some("org.telegram.desktop"));
        assert_eq!(icon("Firefox"), Some("firefox"));
        assert_eq!(icon("notify-send"), None);
    }
}
