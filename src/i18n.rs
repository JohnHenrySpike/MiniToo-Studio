//! Interface languages and date/time formats.
//!
//! Texts live in catalogs, one file per language: `locales/<code>.lang` in the sources (built
//! in: English and Russian) and `~/.config/minitoo-studio/locales/<code>.lang` for anyone's own
//! translation (see `locales/README.md`). A file in the configuration directory adds a language
//! or, with the code of a built-in one, overrides its strings. A key missing from a catalog is
//! taken from English.
//!
//! ```text
//! # comment
//! @name = English          the language as it calls itself (the list in Settings)
//! @plural = en             plural rules (en, fr, ru, pl, cs, none); by default from the code
//! nav.image = Image
//! log.saved = Saved {file}                       {name} — a value put in by the program
//! modes.sessions[one] = {n} session              plural forms: zero, one, two, few, many, other
//! modes.sessions[other] = {n} sessions
//! ```
//!
//! A value may be wrapped in double quotes to keep spaces at its ends; `\n` is a line break.
//!
//! In code: `tr!("key")` → `&'static str`, `tr!("key", file = name)` → `String`,
//! `trn!("key", count)` → `String` with the plural form for `count` (also as `{n}`).
//! Until [`set_language`] is called (tests, tools) Russian is used.

use chrono::{Datelike, Timelike};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, AtomicU8, Ordering};

/// Built-in catalogs: (code, text).
const BUILTIN: [(&str, &str); 2] = [("en", include_str!("../locales/en.lang")), ("ru", include_str!("../locales/ru.lang"))];
const DEFAULT: &str = "ru";
const FALLBACK: &str = "en";

// ---------------------------------------------------------------------- catalogs

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluralRule {
    /// one: 1; other
    En,
    /// one: 0, 1; other
    Fr,
    /// one: 1, 21, 31…; few: 2–4, 22–24…; many: the rest (Russian, Ukrainian, Belarusian)
    Ru,
    /// one: 1; few: 2–4, 22–24…; many: the rest
    Pl,
    /// one: 1; few: 2–4; other
    Cs,
    /// other only (Japanese, Chinese, Korean, …)
    None,
}

impl PluralRule {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "en" => PluralRule::En,
            "fr" => PluralRule::Fr,
            "ru" | "uk" | "be" => PluralRule::Ru,
            "pl" => PluralRule::Pl,
            "cs" | "sk" => PluralRule::Cs,
            "none" | "ja" | "zh" | "ko" | "vi" | "th" | "id" | "ms" => PluralRule::None,
            _ => return None,
        })
    }

    /// Rules of a language code (`pt_BR` → `pt`); English ones for an unknown code.
    fn of(code: &str) -> Self {
        match base_code(code).as_str() {
            "fr" | "pt" => PluralRule::Fr,
            other => Self::parse(other).unwrap_or(PluralRule::En),
        }
    }

    pub fn category(self, n: i64) -> &'static str {
        let n = n.unsigned_abs();
        let (m10, m100) = (n % 10, n % 100);
        let few = (2..=4).contains(&m10) && !(12..=14).contains(&m100);
        match self {
            PluralRule::En => {
                if n == 1 {
                    "one"
                } else {
                    "other"
                }
            }
            PluralRule::Fr => {
                if n <= 1 {
                    "one"
                } else {
                    "other"
                }
            }
            PluralRule::Ru => {
                if m10 == 1 && m100 != 11 {
                    "one"
                } else if few {
                    "few"
                } else {
                    "many"
                }
            }
            PluralRule::Pl => {
                if n == 1 {
                    "one"
                } else if few {
                    "few"
                } else {
                    "many"
                }
            }
            PluralRule::Cs => match n {
                1 => "one",
                2..=4 => "few",
                _ => "other",
            },
            PluralRule::None => "other",
        }
    }
}

/// One language. Strings are leaked: there are only a few catalogs and they live as long as
/// the program, which lets `tr` hand out `&'static str`.
#[derive(Debug)]
pub struct Catalog {
    pub code: String,
    pub name: String,
    pub plural: PluralRule,
    strings: HashMap<String, &'static str>,
}

impl Catalog {
    /// Parses catalog text; `code` comes from the file name.
    pub fn parse(code: &str, text: &str) -> Catalog {
        let mut c = Catalog { code: code.to_string(), name: code.to_string(), plural: PluralRule::of(code), strings: HashMap::new() };
        c.merge(text);
        c
    }

    /// Adds (or replaces) the strings of `text`.
    fn merge(&mut self, text: &str) {
        for line in text.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let Some((k, v)) = t.split_once('=') else { continue };
            let (k, v) = (k.trim(), unescape(v.trim()));
            match k {
                "@name" => self.name = v,
                "@plural" => self.plural = PluralRule::parse(&v).unwrap_or(self.plural),
                _ if !k.is_empty() && !k.starts_with('@') => {
                    self.strings.insert(k.to_string(), Box::leak(v.into_boxed_str()));
                }
                _ => {}
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&'static str> {
        self.strings.get(key).copied()
    }

    /// The plural form of `key` for `n`: `key[category]`, then `key[other]`, then `key`.
    pub fn get_plural(&self, key: &str, n: i64) -> Option<&'static str> {
        let cat = self.plural.category(n);
        self.get(&format!("{key}[{cat}]")).or_else(|| self.get(&format!("{key}[other]"))).or_else(|| self.get(key))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.strings.keys().map(|k| k.as_str())
    }
}

fn unescape(v: &str) -> String {
    let v = if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') { &v[1..v.len() - 1] } else { v };
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

/// `en_US.UTF-8` → `en`.
fn base_code(code: &str) -> String {
    code.split(['_', '-', '.', '@']).next().unwrap_or("").trim().to_ascii_lowercase()
}

/// Where people put their own catalogs.
pub fn locales_dir() -> PathBuf {
    crate::settings::config_path().parent().map(|p| p.join("locales")).unwrap_or_else(|| PathBuf::from("locales"))
}

fn builtin(code: &str) -> Option<&'static str> {
    BUILTIN.iter().find(|(c, _)| *c == code).map(|(_, t)| *t)
}

/// The built-in English catalog, the starting point of a translation.
pub fn english_template() -> &'static str {
    builtin(FALLBACK).unwrap_or("")
}

/// User catalogs: (code, path).
fn user_files() -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(locales_dir()) else { return Vec::new() };
    let mut v: Vec<(String, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lang"))
        .filter_map(|p| Some((p.file_stem()?.to_str()?.to_string(), p)))
        .filter(|(code, _)| !code.is_empty())
        .collect();
    v.sort();
    v
}

/// The catalog of `code`: built-in text with the user's file on top; `None` if neither exists.
fn load(code: &str) -> Option<Catalog> {
    let user = user_files().into_iter().find(|(c, _)| c == code).and_then(|(_, p)| std::fs::read_to_string(p).ok());
    let base = builtin(code);
    if base.is_none() && user.is_none() {
        return None;
    }
    let mut c = Catalog::parse(code, base.unwrap_or(""));
    if let Some(text) = user {
        c.merge(&text);
    }
    Some(c)
}

/// Languages to choose from: (code, name), built-in first.
pub fn available() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = BUILTIN.iter().map(|(code, text)| (code.to_string(), Catalog::parse(code, text).name)).collect();
    for (code, path) in user_files() {
        if out.iter().any(|(c, _)| *c == code) {
            continue;
        }
        let name = std::fs::read_to_string(&path).map(|t| Catalog::parse(&code, &t).name).unwrap_or_else(|_| code.clone());
        out.push((code, name));
    }
    out
}

/// The language of the system (`LC_ALL`, `LC_MESSAGES`, `LANG`) if there is a catalog for it,
/// otherwise English.
pub fn system_language() -> String {
    let env = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|k| std::env::var(k).ok()).find(|v| !v.trim().is_empty() && v != "C" && v != "POSIX");
    let code = env.map(|v| base_code(&v)).unwrap_or_default();
    if !code.is_empty() && (builtin(&code).is_some() || user_files().iter().any(|(c, _)| *c == code)) { code } else { FALLBACK.to_string() }
}

// ---------------------------------------------------------------------- current language

static CURRENT: AtomicPtr<Catalog> = AtomicPtr::new(std::ptr::null_mut());

fn leak(c: Catalog) -> &'static Catalog {
    Box::leak(Box::new(c))
}

fn fallback() -> &'static Catalog {
    static F: OnceLock<&'static Catalog> = OnceLock::new();
    F.get_or_init(|| leak(Catalog::parse(FALLBACK, builtin(FALLBACK).unwrap_or(""))))
}

fn default_catalog() -> &'static Catalog {
    static D: OnceLock<&'static Catalog> = OnceLock::new();
    D.get_or_init(|| leak(Catalog::parse(DEFAULT, builtin(DEFAULT).unwrap_or(""))))
}

pub fn current() -> &'static Catalog {
    let p = CURRENT.load(Ordering::Acquire);
    if p.is_null() {
        default_catalog()
    } else {
        // SAFETY: only leaked `&'static Catalog`s are ever stored
        unsafe { &*p }
    }
}

/// Switches the language: a code, or `auto` / empty for the system language. Returns the code
/// in use.
pub fn set_language(code: &str) -> String {
    let code = code.trim();
    let code = if code.is_empty() || code == "auto" { system_language() } else { code.to_string() };
    let catalog = load(&code).unwrap_or_else(|| Catalog::parse(FALLBACK, builtin(FALLBACK).unwrap_or("")));
    let used = catalog.code.clone();
    CURRENT.store(leak(catalog) as *const Catalog as *mut Catalog, Ordering::Release);
    used
}

/// Missing keys are shown as themselves; they are interned once.
fn intern(key: &str) -> &'static str {
    static SEEN: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut seen = SEEN.lock();
    let set = seen.get_or_insert_with(HashSet::new);
    if let Some(k) = set.get(key) {
        return k;
    }
    let k: &'static str = Box::leak(key.to_string().into_boxed_str());
    set.insert(k);
    k
}

/// The text of `key` in the current language.
pub fn tr(key: &str) -> &'static str {
    current().get(key).or_else(|| fallback().get(key)).unwrap_or_else(|| intern(key))
}

/// The plural form of `key` for `n`.
pub fn tr_plural(key: &str, n: i64) -> &'static str {
    current().get_plural(key, n).or_else(|| fallback().get_plural(key, n)).unwrap_or_else(|| intern(key))
}

/// Puts named values into `{name}` places (`{{` / `}}` are literal braces). Unknown names stay.
pub fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('{')
            && let Some(end) = tail.find('}')
            && let Some((_, v)) = args.iter().find(|(n, _)| *n == &tail[1..end])
        {
            out.push_str(&v.to_string());
            rest = &tail[end + 1..];
            continue;
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// `tr!("key")` → `&'static str`; `tr!("key", name = value, …)` → `String`.
#[macro_export]
macro_rules! tr {
    ($key:expr) => {
        $crate::i18n::tr($key)
    };
    ($key:expr, $($name:ident = $val:expr),+ $(,)?) => {
        $crate::i18n::fill($crate::i18n::tr($key), &[$((stringify!($name), &$val as &dyn ::std::fmt::Display)),+])
    };
}

/// `trn!("key", count)` / `trn!("key", count, name = value, …)` → `String`: the plural form for
/// `count`, which is also available as `{n}`.
#[macro_export]
macro_rules! trn {
    ($key:expr, $n:expr $(, $name:ident = $val:expr)* $(,)?) => {{
        let n = $n;
        $crate::i18n::fill(
            $crate::i18n::tr_plural($key, n as i64),
            &[("n", &n as &dyn ::std::fmt::Display) $(, (stringify!($name), &$val as &dyn ::std::fmt::Display))*],
        )
    }};
}

// ---------------------------------------------------------------------- date and time

/// 0: as the language says (`format.hours`), 24, 12.
static HOURS: AtomicU8 = AtomicU8::new(0);
static DATE_PATTERN: Mutex<String> = Mutex::new(String::new());

/// Date presets for Settings: strftime patterns (`""` = as the language says).
pub const DATE_PRESETS: [&str; 6] = ["", "%Y-%m-%d", "%d.%m.%Y", "%d/%m/%Y", "%m/%d/%Y", "%-d %b %Y"];

/// Applies the settings `ui/timeFormat` (`auto`, `24`, `12`) and `ui/dateFormat` (`auto` or a
/// pattern, see [`format_date`]).
pub fn set_formats(hours: &str, date: &str) {
    HOURS.store(
        match hours.trim() {
            "24" => 24,
            "12" => 12,
            _ => 0,
        },
        Ordering::Relaxed,
    );
    let date = date.trim();
    *DATE_PATTERN.lock() = if date == "auto" { String::new() } else { date.to_string() };
}

/// Whether times are shown with AM/PM.
pub fn twelve_hours() -> bool {
    match HOURS.load(Ordering::Relaxed) {
        12 => true,
        24 => false,
        _ => tr("format.hours").trim() == "12",
    }
}

/// The numeric date pattern in use.
pub fn date_pattern() -> String {
    let p = DATE_PATTERN.lock().clone();
    if p.is_empty() { tr("format.date").to_string() } else { p }
}

fn names(key: &str, i: usize) -> String {
    tr(key).split(',').nth(i).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// strftime-like formatting with names from the catalog:
/// `%Y` 2026, `%y` 26, `%m` 09, `%-m` 9, `%d` 05, `%-d` 5, `%B` month (`date.months`, the form
/// used in a date), `%b` short month, `%A` weekday, `%a` short weekday, `%H` `%-H` `%I` `%-I`
/// `%M` `%S` time, `%p` AM/PM, `%%` percent.
pub fn format_date<T: Datelike + Timelike>(t: &T, pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut it = pattern.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let pad = if it.peek() == Some(&'-') {
            it.next();
            false
        } else {
            true
        };
        let num = |v: u32| if pad { format!("{v:02}") } else { v.to_string() };
        let h12 = match t.hour() % 12 {
            0 => 12,
            h => h,
        };
        match it.next() {
            Some('Y') => out.push_str(&t.year().to_string()),
            Some('y') => out.push_str(&format!("{:02}", t.year().rem_euclid(100))),
            Some('m') => out.push_str(&num(t.month())),
            Some('d') => out.push_str(&num(t.day())),
            Some('e') => out.push_str(&t.day().to_string()),
            Some('B') => out.push_str(&names("date.months", t.month0() as usize)),
            Some('b') => out.push_str(&names("date.months.short", t.month0() as usize)),
            Some('A') => out.push_str(&names("date.weekdays", t.weekday().num_days_from_monday() as usize)),
            Some('a') => out.push_str(&names("date.weekdays.short", t.weekday().num_days_from_monday() as usize)),
            Some('H') => out.push_str(&num(t.hour())),
            Some('I') => out.push_str(&num(h12)),
            Some('M') => out.push_str(&num(t.minute())),
            Some('S') => out.push_str(&num(t.second())),
            Some('p') => out.push_str(tr(if t.hour() < 12 { "time.am" } else { "time.pm" })),
            Some('%') => out.push('%'),
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    out
}

/// «20:48», or «8:48 PM» with 12 hours.
pub fn time_hm<T: Datelike + Timelike>(t: &T) -> String {
    format_date(t, if twelve_hours() { "%-I:%M %p" } else { "%H:%M" })
}

/// «20:48:05» / «8:48:05 PM».
pub fn time_hms<T: Datelike + Timelike>(t: &T) -> String {
    format_date(t, if twelve_hours() { "%-I:%M:%S %p" } else { "%H:%M:%S" })
}

/// Hours and minutes without AM/PM, for big clock digits: «20:48» / «8:48». The marker is
/// [`am_pm`].
pub fn clock_digits<T: Datelike + Timelike>(t: &T) -> String {
    format_date(t, if twelve_hours() { "%-I:%M" } else { "%H:%M" })
}

/// «AM» / «PM» with 12 hours, otherwise `None`.
pub fn am_pm<T: Timelike>(t: &T) -> Option<&'static str> {
    twelve_hours().then(|| tr(if t.hour() < 12 { "time.am" } else { "time.pm" }))
}

/// The numeric date chosen in Settings: «09.10.2026».
pub fn date_numeric<T: Datelike + Timelike>(t: &T) -> String {
    format_date(t, &date_pattern())
}

/// «пятница, 9 октября» / «Friday, October 9» (`date.long`).
pub fn date_long<T: Datelike + Timelike>(t: &T) -> String {
    format_date(t, tr("date.long"))
}

/// «пт, 9 октября» / «Fri, Oct 9» (`date.short`).
pub fn date_short<T: Datelike + Timelike>(t: &T) -> String {
    format_date(t, tr("date.short"))
}

/// «пт» / «Fri».
pub fn weekday_short<T: Datelike>(t: &T) -> String {
    names("date.weekdays.short", t.weekday().num_days_from_monday() as usize)
}

// ---------------------------------------------------------------------- settings

/// Applies the language and formats from the settings at start-up. A configuration written
/// before languages existed (no `ui/language`) keeps Russian.
pub fn init(settings: &mut crate::settings::Settings, had_config: bool) {
    if !settings.contains("ui/language") && had_config {
        settings.set_string("ui/language", "ru");
    }
    set_language(&settings.string("ui/language", "auto"));
    set_formats(&settings.string("ui/timeFormat", "auto"), &settings.string("ui/dateFormat", "auto"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn parses_catalogs() {
        let c = Catalog::parse("xx", "# c\n@name = Тест\n@plural = ru\na = 1\nb = \" spaced \"\nc = line\\nnext\nd[one] = {n} файл\nd[few] = {n} файла\nd[many] = {n} файлов\n");
        assert_eq!(c.name, "Тест");
        assert_eq!(c.plural, PluralRule::Ru);
        assert_eq!(c.get("a"), Some("1"));
        assert_eq!(c.get("b"), Some(" spaced "));
        assert_eq!(c.get("c"), Some("line\nnext"));
        assert_eq!(c.get_plural("d", 1), Some("{n} файл"));
        assert_eq!(c.get_plural("d", 3), Some("{n} файла"));
        assert_eq!(c.get_plural("d", 11), Some("{n} файлов"));
        assert_eq!(c.get_plural("d", 21), Some("{n} файл"));
        assert_eq!(c.get("missing"), None);
    }

    #[test]
    fn plural_rules() {
        let cats = |r: PluralRule| [0, 1, 2, 5, 11, 12, 21, 22, 25, 101, 111].map(|n| r.category(n));
        assert_eq!(cats(PluralRule::En), ["other", "one", "other", "other", "other", "other", "other", "other", "other", "other", "other"]);
        assert_eq!(cats(PluralRule::Ru), ["many", "one", "few", "many", "many", "many", "one", "few", "many", "one", "many"]);
        assert_eq!(cats(PluralRule::Pl)[1..4], ["one", "few", "many"]);
        assert_eq!(cats(PluralRule::Pl)[6], "many");
        assert_eq!(PluralRule::of("pt_BR"), PluralRule::Fr);
        assert_eq!(PluralRule::of("de"), PluralRule::En);
    }

    #[test]
    fn fills_placeholders() {
        assert_eq!(fill("{a} and {b}, {{x}} {c}", &[("a", &1), ("b", &"two")]), "1 and two, {x} {c}");
        assert_eq!(fill("100% {", &[]), "100% {");
    }

    #[test]
    fn builtin_catalogs_match() {
        let en = Catalog::parse("en", builtin("en").unwrap());
        let ru = Catalog::parse("ru", builtin("ru").unwrap());
        let strip = |k: &str| k.split('[').next().unwrap_or(k).to_string();
        let keys = |c: &Catalog| c.keys().map(strip).collect::<std::collections::BTreeSet<_>>();
        let (ke, kr) = (keys(&en), keys(&ru));
        assert!(ke.difference(&kr).next().is_none(), "missing in ru: {:?}", ke.difference(&kr).collect::<Vec<_>>());
        assert!(kr.difference(&ke).next().is_none(), "missing in en: {:?}", kr.difference(&ke).collect::<Vec<_>>());
        // the same placeholders in both
        let places = |s: &str| {
            let mut v: Vec<String> = s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|(n, _)| n.to_string())).filter(|n| !n.is_empty() && n != "n").collect();
            v.sort();
            v.dedup();
            v
        };
        for k in en.keys() {
            let base = strip(k);
            let r = ru.get(k).or_else(|| ru.get_plural(&base, 5)).unwrap_or("");
            assert_eq!(places(en.get(k).unwrap()), places(r), "placeholders of {k}");
        }
        assert_eq!((en.name.as_str(), ru.name.as_str()), ("English", "Русский"));
    }

    #[test]
    fn formats_dates() {
        let t = NaiveDate::from_ymd_opt(2026, 10, 9).unwrap().and_hms_opt(20, 48, 5).unwrap();
        let ru = Catalog::parse("ru", builtin("ru").unwrap());
        assert_eq!(ru.get("date.long").map(|p| p.contains("%A")), Some(true));
        // the default language (Russian) is used: no test switches it
        assert_eq!(format_date(&t, "%Y-%m-%d %-d.%-m %y %H:%M:%S %I %-I %%"), "2026-10-09 9.10 26 20:48:05 08 8 %");
        assert_eq!(format_date(&t, "%A, %-d %B"), "пятница, 9 октября");
        assert_eq!(format_date(&t, "%a %b"), "пт окт");
        assert_eq!(format_date(&t, "%p"), "PM");
    }
}
