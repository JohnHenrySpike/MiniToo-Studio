//! Settings (§15) in a QSettings-compatible INI file, so the configuration of the Qt version is
//! picked up as is: `[group]` sections, nested keys with `\`, string lists as `a, b`.

use std::collections::BTreeMap;
use std::path::PathBuf;

pub const ORG: &str = "minitoo-studio";

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(ORG).join(format!("{ORG}.conf"))
}

/// `~/.local/share/minitoo-studio/MiniToo Studio` (QStandardPaths::AppDataLocation of the Qt app).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(ORG).join("MiniToo Studio")
}

#[derive(Debug, Default, Clone)]
pub struct Settings {
    values: BTreeMap<String, String>,
    path: Option<PathBuf>,
}

impl Settings {
    /// In-memory settings (tests, `--send`).
    pub fn memory() -> Self {
        Self::default()
    }

    pub fn load() -> Self {
        Self::load_from(config_path())
    }

    pub fn load_from(path: PathBuf) -> Self {
        let mut s = Settings { values: BTreeMap::new(), path: Some(path.clone()) };
        if let Ok(text) = std::fs::read_to_string(&path) {
            s.parse(&text);
        }
        s
    }

    fn parse(&mut self, text: &str) {
        let mut group = String::new();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            let t = line.trim();
            if t.is_empty() || t.starts_with(';') || t.starts_with('#') {
                continue;
            }
            if t.starts_with('[') && t.ends_with(']') {
                group = t[1..t.len() - 1].to_string();
                if group == "General" {
                    group.clear();
                }
                continue;
            }
            let Some(eq) = line.find('=') else { continue };
            let key = line[..eq].trim().replace('\\', "/");
            let value = line[eq + 1..].trim().to_string();
            let full = if group.is_empty() { key } else { format!("{group}/{key}") };
            self.values.insert(full, value);
        }
    }

    fn serialize(&self) -> String {
        let mut groups: BTreeMap<String, Vec<(String, &String)>> = BTreeMap::new();
        for (k, v) in &self.values {
            let (g, rest) = match k.split_once('/') {
                Some((g, rest)) => (g.to_string(), rest.replace('/', "\\")),
                None => ("General".to_string(), k.clone()),
            };
            groups.entry(g).or_default().push((rest, v));
        }
        let mut out = String::new();
        for (g, items) in groups {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("[{g}]\n"));
            for (k, v) in items {
                out.push_str(&format!("{k}={v}\n"));
            }
        }
        out
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("conf.tmp");
        if std::fs::write(&tmp, self.serialize()).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    pub fn raw(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|s| s.as_str())
    }

    pub fn string(&self, key: &str, default: &str) -> String {
        match self.values.get(key) {
            Some(v) => unquote(v),
            None => default.to_string(),
        }
    }

    pub fn opt_string(&self, key: &str) -> Option<String> {
        self.values.get(key).map(|v| unquote(v)).filter(|s| !s.is_empty())
    }

    pub fn int(&self, key: &str, default: i64) -> i64 {
        self.values.get(key).and_then(|v| unquote(v).trim().parse::<f64>().ok()).map(|f| f as i64).unwrap_or(default)
    }

    /// Integer clamped into `lo..=hi`.
    pub fn int_in(&self, key: &str, default: i64, lo: i64, hi: i64) -> i64 {
        self.int(key, default).clamp(lo, hi)
    }

    pub fn float(&self, key: &str, default: f64) -> f64 {
        self.values.get(key).and_then(|v| unquote(v).trim().parse::<f64>().ok()).unwrap_or(default)
    }

    pub fn opt_float(&self, key: &str) -> Option<f64> {
        self.values.get(key).and_then(|v| unquote(v).trim().parse::<f64>().ok())
    }

    pub fn bool(&self, key: &str, default: bool) -> bool {
        match self.values.get(key).map(|v| unquote(v).to_ascii_lowercase()) {
            Some(v) if v == "true" || v == "1" => true,
            Some(v) if v == "false" || v == "0" => false,
            _ => default,
        }
    }

    pub fn list(&self, key: &str) -> Vec<String> {
        match self.values.get(key) {
            Some(v) => split_list(v),
            None => Vec::new(),
        }
    }

    pub fn set_string(&mut self, key: &str, value: &str) {
        self.values.insert(key.to_string(), quote(value));
        self.save();
    }

    pub fn set_int(&mut self, key: &str, value: i64) {
        self.values.insert(key.to_string(), value.to_string());
        self.save();
    }

    pub fn set_float(&mut self, key: &str, value: f64) {
        self.values.insert(key.to_string(), value.to_string());
        self.save();
    }

    pub fn set_bool(&mut self, key: &str, value: bool) {
        self.values.insert(key.to_string(), value.to_string());
        self.save();
    }

    pub fn set_list(&mut self, key: &str, items: &[String]) {
        let v = match items.len() {
            0 => "@Invalid()".to_string(),
            1 => {
                // a one-element list is written with a trailing comma by QSettings? no: it is a
                // plain string, which reads back as a one-element list as well
                quote(&items[0])
            }
            _ => items.iter().map(|s| quote(s)).collect::<Vec<_>>().join(", "),
        };
        self.values.insert(key.to_string(), v);
        self.save();
    }

    pub fn remove(&mut self, key: &str) {
        if self.values.remove(key).is_some() {
            self.save();
        }
    }
}

fn needs_quotes(s: &str) -> bool {
    s.contains(',') || s.starts_with(' ') || s.ends_with(' ') || s.contains('"') || s.contains(';') || s.contains('=')
}

fn quote(s: &str) -> String {
    let escaped = escape(s);
    if needs_quotes(s) {
        format!("\"{}\"", escaped.replace('"', "\\\""))
    } else {
        escaped
    }
}

/// QSettings escapes backslashes and control characters; non-ASCII stays UTF-8 in practice
/// for the Qt 6 INI format.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('x') => {
                let mut hex = String::new();
                while let Some(&h) = it.peek() {
                    if h.is_ascii_hexdigit() && hex.len() < 4 {
                        hex.push(h);
                        it.next();
                    } else {
                        break;
                    }
                }
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v == "@Invalid()" {
        return String::new();
    }
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        return unescape(&v[1..v.len() - 1]);
    }
    unescape(v)
}

fn split_list(v: &str) -> Vec<String> {
    let v = v.trim();
    if v.is_empty() || v == "@Invalid()" {
        return Vec::new();
    }
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = v.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => in_quotes = !in_quotes,
            '\\' => {
                cur.push('\\');
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            ',' if !in_quotes => {
                items.push(unescape(cur.trim()));
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    items.push(unescape(cur.trim()));
    items.retain(|s| !s.is_empty());
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_qsettings_ini() {
        let mut s = Settings::memory();
        s.parse(
            "[clock]\ncity=Астана\nlat=51.1801\n\n[live]\nrotation=clock, sysmon, claudestats\nonStart=\n\
             [claude]\nface\\working=/tmp/a.gif\nscenesOff\\alerting=bell, siren\n[device]\nautoConnect=false\n",
        );
        assert_eq!(s.string("clock/city", ""), "Астана");
        assert_eq!(s.float("clock/lat", 0.0), 51.1801);
        assert_eq!(s.list("live/rotation"), vec!["clock", "sysmon", "claudestats"]);
        assert_eq!(s.opt_string("live/onStart"), None);
        assert_eq!(s.string("claude/face/working", ""), "/tmp/a.gif");
        assert_eq!(s.list("claude/scenesOff/alerting"), vec!["bell", "siren"]);
        assert!(!s.bool("device/autoConnect", true));
        assert!(s.bool("ui/closeToTray", true));
    }

    #[test]
    fn roundtrip() {
        let mut s = Settings::memory();
        s.set_list("github/repos", &["cli/cli".into(), "a/b".into()]);
        s.set_string("notify/x", "a, b");
        s.set_list("one", &["solo".into()]);
        s.set_list("none", &[]);
        let text = s.serialize();
        let mut t = Settings::memory();
        t.parse(&text);
        assert_eq!(t.list("github/repos"), vec!["cli/cli", "a/b"]);
        assert_eq!(t.string("notify/x", ""), "a, b");
        assert_eq!(t.list("one"), vec!["solo"]);
        assert!(t.list("none").is_empty());
    }
}
