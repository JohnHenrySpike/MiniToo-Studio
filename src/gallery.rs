//! Local gallery (§6.3): copies of everything opened or sent, deduplicated by SHA-1, with the
//! last crop settings and a "as on the device" thumbnail; plus a user folder as a source.
//! The on-disk layout and `index.json` are the same as in the Qt version.

use crate::api::{Fit, FolderItemView, GalleryFilter, GalleryItemView, NRect};
use crate::frame::Frame;
use crate::media;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const MAX_FOLDER_FILES: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemSettings {
    pub fit: Fit,
    pub pixel_art: bool,
    pub crop: NRect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub file: String,
    pub name: String,
    pub source: String,
    pub added: Option<DateTime<Utc>>,
    pub last_sent: Option<DateTime<Utc>>,
    pub sent: u32,
    pub favorite: bool,
    pub frames: u32,
    pub width: u32,
    pub height: u32,
    pub settings: Option<ItemSettings>,
}

pub struct Gallery {
    dir: PathBuf,
    items: Vec<Entry>,
    folder: Option<PathBuf>,
    folder_files: Vec<PathBuf>,
    pub filter: GalleryFilter,
    thumbs: HashMap<String, Frame>,
    folder_thumbs: HashMap<PathBuf, Frame>,
    pending_thumbs: HashMap<String, Frame>,
    dirty: bool,
}

/// 20 hex characters of the SHA-1 of the content.
pub fn hash_file(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = sha1_smol::Sha1::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.digest().to_string()[..20].to_string())
}

fn ts(t: &Option<DateTime<Utc>>) -> Option<String> {
    t.map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn parse_ts(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|t| t.with_timezone(&Utc))
}

fn rect_from_json(v: &Value) -> NRect {
    let Some(a) = v.as_array().filter(|a| a.len() == 4) else { return NRect::FULL };
    let f: Vec<f64> = a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect();
    if f[2] <= 0.0 || f[3] <= 0.0 {
        return NRect::FULL;
    }
    // intersect with the unit square
    let x0 = f[0].max(0.0);
    let y0 = f[1].max(0.0);
    let x1 = (f[0] + f[2]).min(1.0);
    let y1 = (f[1] + f[3]).min(1.0);
    if x1 <= x0 || y1 <= y0 {
        return NRect::FULL;
    }
    NRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

fn save_png(path: &Path, frame: &Frame) -> bool {
    let tmp = path.with_extension("png.tmp");
    std::fs::write(&tmp, frame.png()).is_ok() && std::fs::rename(&tmp, path).is_ok()
}

fn load_png(path: &Path) -> Option<Frame> {
    let img = image::open(path).ok()?.into_rgba8();
    Some(Frame::from_image(&img))
}

/// Thumbnail of a file "as on the device": its settings or the defaults (5:4 crop, pixel art
/// when it fits the screen). Only the first frame is decoded.
pub fn render_file(path: &Path, settings: Option<ItemSettings>) -> Option<Frame> {
    let a = media::load(path, 1).ok()?;
    let src = a.frames.first()?;
    Some(match settings {
        Some(s) => media::render(src, s.crop, s.fit, s.pixel_art),
        None => media::render(src, media::default_crop(a.width, a.height), Fit::Crop, media::auto_pixel_art(a.width, a.height)),
    })
}

impl Gallery {
    pub fn default_dir() -> PathBuf {
        crate::settings::data_dir().join("gallery")
    }

    pub fn open(dir: PathBuf) -> Self {
        let mut g = Gallery {
            dir,
            items: Vec::new(),
            folder: None,
            folder_files: Vec::new(),
            filter: GalleryFilter::All,
            thumbs: HashMap::new(),
            folder_thumbs: HashMap::new(),
            pending_thumbs: HashMap::new(),
            dirty: false,
        };
        g.load();
        g
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn load(&mut self) {
        let Ok(text) = std::fs::read_to_string(self.dir.join("index.json")) else { return };
        let Ok(root) = serde_json::from_str::<Value>(&text) else { return };
        let mut seen = std::collections::HashSet::new();
        for o in root.get("items").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            let s = |k: &str| o.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let id = s("id");
            let file = s("file");
            if id.is_empty() || file.is_empty() || file.contains('/') || seen.contains(&id) {
                continue;
            }
            if !self.dir.join(&file).exists() {
                continue; // copy removed by hand
            }
            seen.insert(id.clone());
            let settings = o.get("settings").filter(|v| v.is_object()).map(|so| ItemSettings {
                fit: Fit::from_i64(so.get("fit").and_then(|v| v.as_i64()).unwrap_or(0).clamp(0, 2)),
                pixel_art: so.get("pixelArt").and_then(|v| v.as_bool()).unwrap_or(false),
                crop: rect_from_json(so.get("crop").unwrap_or(&Value::Null)),
            });
            self.items.push(Entry {
                name: o.get("name").and_then(|v| v.as_str()).map(String::from).unwrap_or_else(|| file.clone()),
                source: s("source"),
                added: o.get("added").and_then(parse_ts),
                last_sent: o.get("lastSent").and_then(parse_ts),
                sent: o.get("sent").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                favorite: o.get("favorite").and_then(|v| v.as_bool()).unwrap_or(false),
                frames: o.get("frames").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as u32,
                width: o.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                height: o.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                id,
                file,
                settings,
            });
        }
        self.folder = root.get("folder").and_then(|v| v.as_str()).map(PathBuf::from).filter(|p| p.is_dir());
    }

    pub fn save(&self) -> bool {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|e| {
                let mut o = json!({
                    "id": e.id, "file": e.file, "name": e.name, "source": e.source,
                    "added": ts(&e.added).unwrap_or_default(), "sent": e.sent, "favorite": e.favorite,
                    "frames": e.frames, "width": e.width, "height": e.height,
                });
                if let Some(t) = ts(&e.last_sent) {
                    o["lastSent"] = json!(t);
                }
                if let Some(s) = e.settings {
                    o["settings"] = json!({
                        "fit": s.fit as i64, "pixelArt": s.pixel_art,
                        "crop": [s.crop.x, s.crop.y, s.crop.w, s.crop.h],
                    });
                }
                o
            })
            .collect();
        let mut root = json!({ "version": 1, "items": items });
        if let Some(f) = &self.folder {
            root["folder"] = json!(f.to_string_lossy());
        }
        if std::fs::create_dir_all(&self.dir).is_err() {
            return false;
        }
        let tmp = self.dir.join("index.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&root).unwrap_or_default()).is_ok()
            && std::fs::rename(&tmp, self.dir.join("index.json")).is_ok()
    }

    /// Something changed: the controller flushes 700 ms later.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Writes pending thumbnails and the index.
    pub fn flush(&mut self) {
        if !self.pending_thumbs.is_empty() {
            let _ = std::fs::create_dir_all(self.dir.join("thumbs"));
        }
        for (id, frame) in std::mem::take(&mut self.pending_thumbs) {
            if self.index_of(&id).is_some() {
                save_png(&self.thumb_path(&id), &frame);
            }
        }
        self.save();
        self.dirty = false;
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.items.iter().position(|e| e.id == id)
    }

    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.items.iter().find(|e| e.id == id)
    }

    pub fn items(&self) -> &[Entry] {
        &self.items
    }

    pub fn file_path(&self, id: &str) -> Option<PathBuf> {
        self.entry(id).map(|e| self.dir.join(&e.file))
    }

    pub fn thumb_path(&self, id: &str) -> PathBuf {
        self.dir.join("thumbs").join(format!("{id}.png"))
    }

    /// Adds a file (copying it into the gallery). Known content returns the existing id.
    /// `frames`/`size` are probed when not given.
    pub fn add(&mut self, path: &Path, info: Option<(u32, u32, u32)>) -> Option<String> {
        let abs = std::fs::canonicalize(path).ok()?;
        if !abs.is_file() {
            return None;
        }
        let id = hash_file(&abs)?;
        if self.index_of(&id).is_some() {
            return Some(id);
        }
        let (width, height, frames) = match info {
            Some(i) => i,
            None => media::probe(&abs)?,
        };
        let _ = std::fs::create_dir_all(self.dir.join("thumbs"));
        let mut suffix = abs.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if suffix.is_empty() || suffix.chars().count() > 5 {
            suffix = "img".into();
        }
        let file = format!("{id}.{suffix}");
        let dest = self.dir.join(&file);
        if std::fs::canonicalize(&dest).ok().as_deref() != Some(abs.as_path()) {
            let _ = std::fs::remove_file(&dest);
            std::fs::copy(&abs, &dest).ok()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o644));
            }
        }
        self.items.insert(
            0,
            Entry {
                id: id.clone(),
                file,
                name: abs.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                source: abs.to_string_lossy().into_owned(),
                added: Some(Utc::now()),
                last_sent: None,
                sent: 0,
                favorite: false,
                frames: frames.max(1),
                width,
                height,
                settings: None,
            },
        );
        self.dirty = true;
        Some(id)
    }

    /// Stores the settings; a new thumbnail is written on the next flush when they changed
    /// (or no thumbnail exists yet).
    pub fn update(&mut self, id: &str, settings: ItemSettings, rendered: Option<Frame>) {
        let Some(i) = self.index_of(id) else { return };
        let changed = self.items[i].settings != Some(settings);
        self.items[i].settings = Some(settings);
        let need_thumb = rendered.is_some() && (changed || !self.thumb_path(id).exists());
        if let (true, Some(f)) = (need_thumb, rendered) {
            self.thumbs.insert(id.to_string(), f.clone());
            self.pending_thumbs.insert(id.to_string(), f);
        }
        if changed || need_thumb {
            self.dirty = true;
        }
    }

    pub fn mark_sent(&mut self, id: &str) {
        if let Some(i) = self.index_of(id) {
            self.items[i].sent += 1;
            self.items[i].last_sent = Some(Utc::now());
            self.dirty = true;
        }
    }

    /// Deletes our copy and its thumbnail; never the user's original.
    pub fn remove(&mut self, id: &str) {
        let Some(i) = self.index_of(id) else { return };
        let file = self.dir.join(&self.items[i].file);
        if file.parent() == Some(self.dir.as_path()) {
            let _ = std::fs::remove_file(&file);
        }
        let _ = std::fs::remove_file(self.thumb_path(id));
        self.pending_thumbs.remove(id);
        self.thumbs.remove(id);
        self.items.remove(i);
        self.dirty = true;
    }

    pub fn set_favorite(&mut self, id: &str, on: bool) {
        if let Some(i) = self.index_of(id) {
            if self.items[i].favorite != on {
                self.items[i].favorite = on;
                self.dirty = true;
            }
        }
    }

    /// Files and folders (top-level files only). Returns the ids added or found.
    pub fn add_paths(&mut self, paths: &[PathBuf]) -> Vec<String> {
        let mut ids = Vec::new();
        for p in paths {
            if p.is_dir() {
                let mut files: Vec<PathBuf> = std::fs::read_dir(p)
                    .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|f| f.is_file() && media::is_image_file(f)).collect())
                    .unwrap_or_default();
                files.sort();
                for f in files {
                    ids.extend(self.add(&f, None));
                }
            } else {
                ids.extend(self.add(p, None));
            }
        }
        ids
    }

    // ---------------------------------------------------------------- thumbnails

    /// Cached thumbnail, loading `thumbs/<id>.png` on first use.
    pub fn thumb(&mut self, id: &str) -> Option<Frame> {
        if let Some(f) = self.thumbs.get(id) {
            return Some(f.clone());
        }
        let f = load_png(&self.thumb_path(id))?;
        self.thumbs.insert(id.to_string(), f.clone());
        Some(f)
    }

    /// Ids whose thumbnail does not exist yet (rendered lazily with default settings).
    pub fn missing_thumbs(&self) -> Vec<(String, PathBuf, Option<ItemSettings>)> {
        self.items
            .iter()
            .filter(|e| !self.thumbs.contains_key(&e.id) && !self.thumb_path(&e.id).exists())
            .map(|e| (e.id.clone(), self.dir.join(&e.file), e.settings))
            .collect()
    }

    /// A lazily rendered thumbnail arrived.
    pub fn store_thumb(&mut self, id: &str, frame: Frame) {
        if self.index_of(id).is_none() {
            return;
        }
        self.thumbs.insert(id.to_string(), frame.clone());
        let _ = std::fs::create_dir_all(self.dir.join("thumbs"));
        save_png(&self.thumb_path(id), &frame);
    }

    // ---------------------------------------------------------------- folder source

    pub fn folder(&self) -> Option<&Path> {
        self.folder.as_deref()
    }

    pub fn set_folder(&mut self, folder: Option<PathBuf>) {
        self.folder = folder.filter(|p| p.is_dir()).map(|p| std::fs::canonicalize(&p).unwrap_or(p));
        self.folder_files.clear();
        self.dirty = true;
        self.rescan_folder();
    }

    /// Top-level image files, newest first, up to 1000.
    pub fn rescan_folder(&mut self) {
        self.folder_files.clear();
        let Some(dir) = &self.folder else { return };
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                    .map(|e| e.path())
                    .filter(|p| media::is_image_file(p))
                    .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p))
                    .collect()
            })
            .unwrap_or_default();
        files.sort_by(|a, b| b.0.cmp(&a.0));
        self.folder_files = files.into_iter().take(MAX_FOLDER_FILES).map(|(_, p)| p).collect();
    }

    pub fn folder_files(&self) -> &[PathBuf] {
        &self.folder_files
    }

    /// `cache/<sha1(path|size|mtime)>.png`
    pub fn folder_cache_path(&self, path: &Path) -> PathBuf {
        let md = std::fs::metadata(path).ok();
        let size = md.as_ref().map(|m| m.len()).unwrap_or(0);
        let mtime = md
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let key = format!("{}|{size}|{mtime}", path.display());
        self.dir.join("cache").join(format!("{}.png", sha1_smol::Sha1::from(key).digest()))
    }

    pub fn folder_thumb(&mut self, path: &Path) -> Option<Frame> {
        if let Some(f) = self.folder_thumbs.get(path) {
            return Some(f.clone());
        }
        let f = load_png(&self.folder_cache_path(path))?;
        self.folder_thumbs.insert(path.to_path_buf(), f.clone());
        Some(f)
    }

    pub fn store_folder_thumb(&mut self, path: &Path, frame: Frame) {
        let cache = self.folder_cache_path(path);
        let _ = std::fs::create_dir_all(self.dir.join("cache"));
        save_png(&cache, &frame);
        self.folder_thumbs.insert(path.to_path_buf(), frame);
    }

    /// Folder files without a cached thumbnail.
    pub fn missing_folder_thumbs(&self) -> Vec<PathBuf> {
        self.folder_files
            .iter()
            .filter(|p| !self.folder_thumbs.contains_key(*p) && !self.folder_cache_path(p).exists())
            .cloned()
            .collect()
    }

    // ---------------------------------------------------------------- views

    pub fn view_items(&mut self) -> Vec<GalleryItemView> {
        let ids: Vec<String> = self.items.iter().map(|e| e.id.clone()).collect();
        for id in &ids {
            let _ = self.thumb(id);
        }
        self.items
            .iter()
            .map(|e| GalleryItemView {
                id: e.id.clone(),
                name: e.name.clone(),
                file: self.dir.join(&e.file),
                width: e.width,
                height: e.height,
                frames: e.frames,
                sent: e.sent,
                favorite: e.favorite,
                thumb: self.thumbs.get(&e.id).cloned(),
            })
            .collect()
    }

    pub fn view_folder(&mut self) -> Vec<FolderItemView> {
        let files = self.folder_files.clone();
        files
            .iter()
            .map(|p| FolderItemView {
                path: p.clone(),
                name: p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                thumb: self.folder_thumb(p),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_roundtrip_dedupe_and_remove() {
        let root = std::env::temp_dir().join(format!("minitoo-gallery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let png = src.join("Cat.PNG");
        image::RgbaImage::from_pixel(320, 256, image::Rgba([1, 2, 3, 255])).save_with_format(&png, image::ImageFormat::Png).unwrap();
        let dir = root.join("gallery");
        let mut g = Gallery::open(dir.clone());
        let id = g.add(&png, None).unwrap();
        assert_eq!(id.len(), 20);
        assert_eq!(g.add(&png, None).unwrap(), id, "no duplicate by hash");
        assert_eq!(g.items().len(), 1);
        let e = g.entry(&id).unwrap().clone();
        assert_eq!((e.width, e.height, e.frames), (320, 256, 1));
        assert_eq!(e.file, format!("{id}.png"));
        let s = ItemSettings { fit: Fit::Fit, pixel_art: true, crop: NRect { x: 0.1, y: 0.0, w: 0.5, h: 0.5 } };
        g.update(&id, s, Some(Frame::solid(9, 9, 9)));
        g.mark_sent(&id);
        g.set_favorite(&id, true);
        g.flush();
        assert!(g.thumb_path(&id).exists());

        let text = std::fs::read_to_string(dir.join("index.json")).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["items"][0]["settings"]["fit"], 1);
        assert_eq!(v["items"][0]["sent"], 1);
        assert!(v["items"][0]["lastSent"].as_str().unwrap().ends_with('Z'));

        let mut g2 = Gallery::open(dir.clone());
        let e2 = g2.entry(&id).unwrap();
        assert_eq!(e2.settings, Some(s));
        assert!(e2.favorite);
        assert_eq!(g2.thumb(&id).unwrap().pixel(0, 0), [9, 9, 9]);
        g2.remove(&id);
        assert!(!dir.join(format!("{id}.png")).exists());
        assert!(png.exists(), "the original is never touched");

        g2.set_folder(Some(src.clone()));
        assert_eq!(g2.folder_files().len(), 1);
        assert_eq!(g2.missing_folder_thumbs().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
