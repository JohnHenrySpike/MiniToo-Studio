//! Freedesktop icon theme lookup for notification cards (§10).
//!
//! `find_icon` accepts an absolute path / `file://` URL (loaded directly) or a theme icon name.
//! Names are looked up in the current theme (KDE: `~/.config/kdeglobals` `[Icons] Theme=`), its
//! `Inherits=` chain, then breeze, Adwaita and hicolor, and finally `/usr/share/pixmaps`. PNG and
//! SVG are supported (SVG through resvg). Results, misses included, are cached.

use image::RgbaImage;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Finds an icon by theme name or file path, rendered to `size`×`size` straight RGBA.
pub fn find_icon(name: &str, size: u32) -> Option<RgbaImage> {
    let name = name.trim();
    if name.is_empty() || size == 0 {
        return None;
    }
    static CACHE: OnceLock<Mutex<HashMap<(String, u32), Option<RgbaImage>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (name.to_string(), size);
    if let Some(hit) = cache.lock().get(&key) {
        return hit.clone();
    }
    let found = lookup(name, size);
    cache.lock().insert(key, found.clone());
    found
}

fn lookup(name: &str, size: u32) -> Option<RgbaImage> {
    if let Some(path) = as_path(name) {
        return load_file(&path, size);
    }
    #[cfg(target_os = "linux")]
    {
        let path = theme_lookup(name, size)?;
        load_file(&path, size)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// An absolute path or a `file://` URL (percent-decoded).
fn as_path(name: &str) -> Option<PathBuf> {
    if let Some(rest) = name.strip_prefix("file://") {
        return Some(PathBuf::from(percent_decode(rest)));
    }
    let p = Path::new(name);
    if p.is_absolute() {
        return Some(p.to_path_buf());
    }
    None
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Loads a PNG/JPEG/… or SVG file and scales it to `size`×`size` (keeping the aspect ratio,
/// centred on a transparent square).
pub fn load_file(path: &Path, size: u32) -> Option<RgbaImage> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let data = std::fs::read(path).ok()?;
    if ext == "svg" || (ext.is_empty() && data.trim_ascii_start().starts_with(b"<")) {
        return render_svg(&data, size);
    }
    let img = image::load_from_memory(&data).ok()?.to_rgba8();
    Some(fit_square(&img, size))
}

/// Scales to fit `size`×`size`, keeping the aspect ratio, centred on a transparent square.
pub fn fit_square(img: &RgbaImage, size: u32) -> RgbaImage {
    if img.width() == size && img.height() == size {
        return img.clone();
    }
    let (w, h) = (img.width().max(1) as f32, img.height().max(1) as f32);
    let k = size as f32 / w.max(h);
    let nw = ((w * k).round() as u32).clamp(1, size);
    let nh = ((h * k).round() as u32).clamp(1, size);
    let scaled = image::imageops::resize(img, nw, nh, image::imageops::FilterType::Lanczos3);
    let mut out = RgbaImage::new(size, size);
    image::imageops::overlay(&mut out, &scaled, ((size - nw) / 2) as i64, ((size - nh) / 2) as i64);
    out
}

fn render_svg(data: &[u8], size: u32) -> Option<RgbaImage> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_data(data, &usvg::Options::default()).ok()?;
    let s = tree.size();
    let (w, h) = (s.width().max(1.0), s.height().max(1.0));
    let k = size as f32 / w.max(h);
    let mut pm = tiny_skia::Pixmap::new(size, size)?;
    let dx = (size as f32 - w * k) / 2.0;
    let dy = (size as f32 - h * k) / 2.0;
    resvg::render(&tree, tiny_skia::Transform::from_row(k, 0.0, 0.0, k, dx, dy), &mut pm.as_mut());
    RgbaImage::from_raw(size, size, pm.take_demultiplied())
}

// ---------------------------------------------------------------------- theme lookup

#[cfg(target_os = "linux")]
fn theme_lookup(name: &str, size: u32) -> Option<PathBuf> {
    let bases = base_dirs();
    let chain = theme_chain(&bases);
    // the exact name first, then the generic fallbacks ("a-b-c" → "a-b" → "a")
    let mut candidate = name.to_string();
    loop {
        for theme in chain.iter() {
            if let Some(p) = find_in_theme(theme, &candidate, size) {
                return Some(p);
            }
        }
        if let Some(p) = find_pixmap(&candidate) {
            return Some(p);
        }
        match candidate.rfind('-') {
            Some(i) if i > 0 => candidate.truncate(i),
            _ => return None,
        }
    }
}

#[cfg(target_os = "linux")]
fn base_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = dirs::home_dir() {
        v.push(home.join(".icons"));
    }
    if let Some(data) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()) {
        v.push(data.join("icons"));
    } else if let Some(home) = dirs::home_dir() {
        v.push(home.join(".local/share/icons"));
    }
    let xdg = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    for d in xdg.split(':').filter(|d| !d.is_empty()) {
        v.push(Path::new(d).join("icons"));
    }
    v.push(PathBuf::from("/usr/local/share/icons"));
    v.push(PathBuf::from("/usr/share/icons"));
    let mut seen = std::collections::HashSet::new();
    v.retain(|p| seen.insert(p.clone()));
    v
}

#[cfg(target_os = "linux")]
fn find_pixmap(name: &str) -> Option<PathBuf> {
    for ext in ["png", "svg"] {
        let p = Path::new("/usr/share/pixmaps").join(format!("{name}.{ext}"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// The current theme name from `kdeglobals` (`[Icons] Theme=`).
#[cfg(target_os = "linux")]
fn current_theme() -> Option<String> {
    let cfg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))?;
    let text = std::fs::read_to_string(cfg.join("kdeglobals")).ok()?;
    ini_value(&text, "Icons", "Theme")
}

#[cfg(target_os = "linux")]
fn ini_value(text: &str, group: &str, key: &str) -> Option<String> {
    let mut in_group = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_group = t == format!("[{group}]");
            continue;
        }
        if !in_group {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            if k.trim() == key {
                let v = v.trim();
                return (!v.is_empty()).then(|| v.to_string());
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct Theme {
    /// theme directories (one per base dir that has it)
    roots: Vec<PathBuf>,
    dirs: Vec<ThemeDir>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct ThemeDir {
    path: String,
    size: u32,
    scale: u32,
    scalable: bool,
    min: u32,
    max: u32,
}

/// The current theme with its `Inherits=` chain, then breeze, Adwaita and hicolor. Cached.
#[cfg(target_os = "linux")]
fn theme_chain(bases: &[PathBuf]) -> std::sync::Arc<Vec<Theme>> {
    static CHAIN: OnceLock<std::sync::Arc<Vec<Theme>>> = OnceLock::new();
    CHAIN
        .get_or_init(|| {
            let mut names: Vec<String> = Vec::new();
            let mut queue: Vec<String> = current_theme().into_iter().collect();
            queue.extend(["breeze", "Adwaita"].map(String::from));
            let mut themes = Vec::new();
            let mut i = 0;
            while i < queue.len() && themes.len() < 16 {
                let n = queue[i].clone();
                i += 1;
                if names.contains(&n) || n == "hicolor" {
                    continue;
                }
                names.push(n.clone());
                if let Some((theme, inherits)) = load_theme(bases, &n) {
                    themes.push(theme);
                    // inherited themes go right after this one, before the generic fallbacks
                    for (k, parent) in inherits.into_iter().enumerate() {
                        queue.insert(i + k, parent);
                    }
                }
            }
            // hicolor is the fallback of every theme: always last
            if let Some((theme, _)) = load_theme(bases, "hicolor") {
                themes.push(theme);
            }
            std::sync::Arc::new(themes)
        })
        .clone()
}

#[cfg(target_os = "linux")]
fn load_theme(bases: &[PathBuf], name: &str) -> Option<(Theme, Vec<String>)> {
    let roots: Vec<PathBuf> = bases.iter().map(|b| b.join(name)).filter(|p| p.is_dir()).collect();
    let index = roots.iter().find_map(|r| std::fs::read_to_string(r.join("index.theme")).ok())?;
    let (dirs, inherits) = parse_index(&index);
    Some((Theme { roots, dirs }, inherits))
}

#[cfg(target_os = "linux")]
fn parse_index(text: &str) -> (Vec<ThemeDir>, Vec<String>) {
    let mut groups: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut cur = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            cur = t[1..t.len() - 1].to_string();
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            groups.entry(cur.clone()).or_default().insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let main = groups.get("Icon Theme").cloned().unwrap_or_default();
    let list = |k: &str| -> Vec<String> {
        main.get(k)
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default()
    };
    let mut names = list("Directories");
    names.extend(list("ScaledDirectories"));
    let mut dirs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for d in names {
        if !seen.insert(d.clone()) {
            continue;
        }
        let Some(g) = groups.get(&d) else { continue };
        let num = |k: &str| g.get(k).and_then(|v| v.parse::<u32>().ok());
        let Some(size) = num("Size") else { continue };
        let ty = g.get("Type").map(|s| s.as_str()).unwrap_or("Threshold");
        let threshold = num("Threshold").unwrap_or(2);
        let (min, max) = match ty {
            "Scalable" => (num("MinSize").unwrap_or(size), num("MaxSize").unwrap_or(size)),
            "Fixed" => (size, size),
            _ => (size.saturating_sub(threshold), size + threshold),
        };
        dirs.push(ThemeDir { path: d, size, scale: num("Scale").unwrap_or(1).max(1), scalable: ty == "Scalable", min, max });
    }
    (dirs, list("Inherits"))
}

/// Best file for `name` in one theme: the smallest directory at least `size` px (scalable
/// ones count as their max size), otherwise the biggest available.
#[cfg(target_os = "linux")]
fn find_in_theme(theme: &Theme, name: &str, size: u32) -> Option<PathBuf> {
    let mut best: Option<((u8, i64), PathBuf)> = None;
    for d in &theme.dirs {
        for root in &theme.roots {
            for ext in ["png", "svg"] {
                let p = root.join(&d.path).join(format!("{name}.{ext}"));
                if !p.is_file() {
                    continue;
                }
                let px = (if d.scalable && ext == "svg" { d.max.max(d.size) } else { d.size }) * d.scale;
                let fits = d.scalable && ext == "svg" && d.min * d.scale <= size && size <= d.max * d.scale;
                // lower is better: (class, distance)
                let score = if fits {
                    (0, 0)
                } else if px >= size {
                    (1, (px - size) as i64 * 2 + (ext == "png") as i64)
                } else {
                    (2, (size - px) as i64 * 2 + (ext == "png") as i64)
                };
                if best.as_ref().is_none_or(|(s, _)| score < *s) {
                    best = Some((score, p));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_absolute_png_and_svg() {
        let dir = std::env::temp_dir().join(format!("minitoo-icon-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        let mut img = RgbaImage::new(10, 20);
        for p in img.pixels_mut() {
            *p = image::Rgba([255, 0, 0, 255]);
        }
        img.save(&png).unwrap();
        let got = find_icon(png.to_str().unwrap(), 28).unwrap();
        assert_eq!((got.width(), got.height()), (28, 28));
        assert_eq!(got.get_pixel(14, 14).0, [255, 0, 0, 255]);
        assert_eq!(got.get_pixel(0, 14).0[3], 0);
        let svg = dir.join("b.svg");
        std::fs::write(
            &svg,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="#00ff00"/></svg>"##,
        )
        .unwrap();
        let got = find_icon(&format!("file://{}", svg.display()), 32).unwrap();
        assert_eq!(got.get_pixel(16, 16).0, [0, 255, 0, 255]);
        assert!(find_icon("/nonexistent/x.png", 28).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_index_theme() {
        let (dirs, inherits) = parse_index(
            "[Icon Theme]\nName=X\nInherits=breeze,hicolor\nDirectories=apps/48,apps/scalable\n\n\
             [apps/48]\nSize=48\nType=Fixed\n\n[apps/scalable]\nSize=64\nMinSize=8\nMaxSize=512\nType=Scalable\n",
        );
        assert_eq!(inherits, vec!["breeze", "hicolor"]);
        assert_eq!(dirs.len(), 2);
        assert!(dirs[1].scalable && dirs[1].max == 512);
        assert_eq!(percent_decode("/a%20b.png"), "/a b.png");
    }
}
