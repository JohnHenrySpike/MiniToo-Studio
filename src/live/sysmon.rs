//! Системный монитор (`sysmon`, §8.3): CPU, GPU, RAM, VRAM and temperatures every 2 s.
//!
//! Linux reads `/proc` and `/sys` (and runs `nvidia-smi` when there is no AMD GPU); other systems
//! use the `sysinfo` crate. Sampling runs on the blocking pool, never on the controller loop.

use super::{LiveMode, ModeCx, ModeMsg};
use crate::canvas::{r, Align, Canvas};
use crate::color::Color;
use crate::fonts::FontSpec;
use crate::frame::Frame;
use std::collections::VecDeque;
use std::time::Duration;

const TICK: u64 = 1;
const EVERY: Duration = Duration::from_secs(2);
const HISTORY: usize = 48;

/// One reading of the sensors.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// CPU load, % (None on the first sample, before there is a delta)
    pub cpu: Option<f64>,
    pub cpu_temp: Option<f64>,
    pub mem_used: f64,
    pub mem_total: f64,
    /// GPU load %, temperature, VRAM used / total in bytes
    pub gpu: Option<f64>,
    pub gpu_temp: Option<f64>,
    pub vram: Option<(f64, f64)>,
}

/// State carried between samples (CPU counters, the sysinfo handle).
#[derive(Default)]
struct Sampler {
    #[cfg(target_os = "linux")]
    prev: Option<(u64, u64)>,
    #[cfg(target_os = "linux")]
    no_nvidia: bool,
    #[cfg(not(target_os = "linux"))]
    sys: Option<sysinfo::System>,
}

struct SampleMsg {
    sampler: Sampler,
    reading: Reading,
}

pub struct SystemMonitor {
    sampler: Option<Sampler>,
    cpu: f64,
    cpu_temp: Option<f64>,
    gpu: Option<f64>,
    gpu_temp: Option<f64>,
    mem_used: f64,
    mem_total: f64,
    vram: Option<(f64, f64)>,
    history: VecDeque<f64>,
}

impl Default for SystemMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemMonitor {
    pub fn new() -> Self {
        SystemMonitor {
            sampler: Some(Sampler::default()),
            cpu: 0.0,
            cpu_temp: None,
            gpu: None,
            gpu_temp: None,
            mem_used: 0.0,
            mem_total: 0.0,
            vram: None,
            history: VecDeque::new(),
        }
    }

    fn sample(&mut self, cx: &mut ModeCx) {
        // one sample at a time: the sampler travels to the blocking pool and back
        let Some(mut sampler) = self.sampler.take() else { return };
        cx.spawn_blocking(move || {
            let reading = sampler.read();
            Box::new(SampleMsg { sampler, reading }) as ModeMsg
        });
    }

    /// Applies a reading (also used to render offscreen).
    pub fn apply(&mut self, r: &Reading) {
        if let Some(c) = r.cpu {
            self.cpu = c;
        }
        self.history.push_back(self.cpu);
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        self.cpu_temp = r.cpu_temp;
        self.mem_used = r.mem_used;
        self.mem_total = r.mem_total;
        if r.gpu.is_some() {
            self.gpu = r.gpu;
            self.gpu_temp = r.gpu_temp;
        }
        if r.vram.is_some() {
            self.vram = r.vram;
        }
    }

    pub fn frame(&self, now: chrono::NaiveDateTime) -> Frame {
        let mut c = Canvas::device();
        c.fill(Color::hex(0x0a0e16));
        c.text(r(8.0, 2.0, 100.0, 18.0), Align::LEFT, "Система", FontSpec::bold(12.0), Color::WHITE);
        c.text(r(100.0, 2.0, 52.0, 18.0), Align::RIGHT, &now.format("%H:%M").to_string(), FontSpec::sans(10.0), Color::hex(0x96a0b4));

        let temp = |t: Option<f64>| t.filter(|t| *t >= 0.0).map(|t| format!("  {}°", t.round() as i64)).unwrap_or_default();
        let mut rows: Vec<(&str, String, f64, Color)> =
            vec![("CPU", format!("{}%{}", self.cpu.round() as i64, temp(self.cpu_temp)), self.cpu / 100.0, Color::hex(0x5ac878))];
        if let Some(g) = self.gpu.filter(|g| *g >= 0.0) {
            rows.push(("GPU", format!("{}%{}", g.round() as i64, temp(self.gpu_temp)), g / 100.0, Color::hex(0xf0aa3c)));
        }
        let ram = if self.mem_total > 0.0 { self.mem_used / self.mem_total } else { 0.0 };
        rows.push(("RAM", format!("{}/{}G", gb(self.mem_used), gb(self.mem_total)), ram, Color::hex(0x6e96f0)));
        if let Some((used, total)) = self.vram.filter(|(_, t)| *t > 0.0) {
            rows.push(("VRAM", format!("{}/{}G", gb(used), gb(total)), used / total, Color::hex(0xb478e6)));
        }
        let text = Color::hex(0xc8cddc);
        let mut y = 22.0;
        for (name, value, v, color) in rows {
            c.text(r(8.0, y, 60.0, 12.0), Align::LEFT, name, FontSpec::bold(9.0), text);
            c.text(r(60.0, y, 92.0, 12.0), Align::RIGHT, &value, FontSpec::sans(9.0), text);
            bar(&mut c, 8.0, y + 13.0, 144.0, 5.0, v, color);
            y += 22.0;
        }
        // CPU history sparkline along the bottom
        if self.history.len() > 1 {
            let (x0, w, top, h) = (8.0, 144.0, 110.0, 14.0);
            let pts: Vec<(f32, f32)> = self
                .history
                .iter()
                .enumerate()
                .map(|(i, v)| (x0 + w * i as f32 / 47.0, top + h - h * (v.clamp(0.0, 100.0) as f32) / 100.0))
                .collect();
            let mut pb = tiny_skia::PathBuilder::new();
            pb.move_to(pts[0].0, pts[0].1);
            for p in &pts[1..] {
                pb.line_to(p.0, p.1);
            }
            if let Some(path) = pb.finish() {
                // QPen default: square cap, bevel join
                c.stroke_path_with(&path, 1.2, Color::hex(0x5ac878), crate::canvas::LineCap::Square, crate::canvas::LineJoin::Bevel);
            }
        }
        c.to_frame()
    }

    fn status(&self) -> String {
        format!("CPU {}%  ·  RAM {} ГБ", self.cpu.round() as i64, gb(self.mem_used))
    }
}

/// The Qt `Canvas::bar`: the filled part is exactly `value · width` wide (no minimum).
fn bar(c: &mut Canvas, x: f32, y: f32, w: f32, h: f32, value: f64, fill: Color) {
    let rad = h / 2.0;
    c.fill_round_rect(x, y, w, h, rad, Color::hex(0x232837));
    let fw = value.clamp(0.0, 1.0) as f32 * w;
    if fw > 0.0 {
        c.fill_round_rect(x, y, fw, h, rad, fill);
    }
}

/// Gigabytes: one decimal below 10 GB, whole numbers above.
pub fn gb(bytes: f64) -> String {
    let v = bytes / (1024.0 * 1024.0 * 1024.0);
    if bytes >= 10.0 * (1u64 << 30) as f64 { format!("{v:.0}") } else { format!("{v:.1}") }
}

impl LiveMode for SystemMonitor {
    fn id(&self) -> &'static str {
        "sysmon"
    }
    fn title(&self) -> &'static str {
        "Системный монитор"
    }
    fn subtitle(&self) -> &'static str {
        "CPU, GPU, память, температуры"
    }
    fn icon(&self) -> &'static str {
        "sysmon"
    }

    fn start(&mut self, cx: &mut ModeCx) {
        self.history.clear();
        // a sample still running when the mode stopped never came back
        if self.sampler.is_none() {
            self.sampler = Some(Sampler::default());
        }
        self.sample(cx);
        cx.timer(EVERY, TICK);
    }

    fn render(&mut self, cx: &mut ModeCx) {
        let now = cx.now().naive_local();
        cx.set_status(self.status());
        cx.publish(self.frame(now));
    }

    fn on_timer(&mut self, cx: &mut ModeCx, token: u64) {
        if token == TICK {
            self.sample(cx);
            cx.timer(EVERY, TICK);
        }
    }

    fn on_message(&mut self, cx: &mut ModeCx, msg: ModeMsg) {
        let Ok(msg) = msg.downcast::<SampleMsg>() else { return };
        let SampleMsg { sampler, reading } = *msg;
        self.sampler = Some(sampler);
        self.apply(&reading);
        self.render(cx);
    }
}

// ---------------------------------------------------------------------- Linux

#[cfg(target_os = "linux")]
impl Sampler {
    fn read(&mut self) -> Reading {
        let mut out = Reading::default();
        if let Some((total, idle)) = std::fs::read_to_string("/proc/stat").ok().and_then(|s| parse_proc_stat(&s)) {
            if let Some((pt, pi)) = self.prev {
                if total > pt {
                    out.cpu = Some(100.0 * (1.0 - idle.saturating_sub(pi) as f64 / (total - pt) as f64));
                }
            }
            self.prev = Some((total, idle));
        }
        if let Some((total, avail)) = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| parse_meminfo(&s)) {
            out.mem_total = total;
            out.mem_used = total - avail;
        }
        out.cpu_temp = cpu_temperature();
        if let Some((busy, used, total)) = amd_gpu() {
            out.gpu = Some(busy);
            out.vram = Some((used, total));
        } else if !self.no_nvidia {
            match nvidia_smi() {
                Ok(Some((util, temp, used, total))) => {
                    out.gpu = Some(util);
                    out.gpu_temp = Some(temp);
                    out.vram = Some((used, total));
                }
                Ok(None) => {}
                Err(_) => self.no_nvidia = true,
            }
        }
        out
    }
}

/// (total, idle) jiffies of the first `/proc/stat` line; idle = 4th + 5th field.
#[cfg(any(target_os = "linux", test))]
pub fn parse_proc_stat(s: &str) -> Option<(u64, u64)> {
    let line = s.lines().next()?;
    let f: Vec<u64> = line.split_whitespace().skip(1).map(|v| v.parse().unwrap_or(0)).collect();
    if f.len() < 7 {
        return None;
    }
    Some((f.iter().sum(), f[3] + f[4]))
}

/// (MemTotal, MemAvailable) in bytes.
#[cfg(any(target_os = "linux", test))]
pub fn parse_meminfo(s: &str) -> Option<(f64, f64)> {
    let mut total = None;
    let mut avail = None;
    for line in s.lines() {
        let kb = || line.split_whitespace().nth(1).and_then(|v| v.parse::<f64>().ok()).map(|v| v * 1024.0);
        if line.starts_with("MemTotal:") {
            total = kb();
        } else if line.starts_with("MemAvailable:") {
            avail = kb();
        }
    }
    Some((total?, avail.unwrap_or(0.0)))
}

#[cfg(target_os = "linux")]
fn read_trim(p: &str) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// CPU package temperature from hwmon (Intel coretemp, AMD k10temp / zenpower).
#[cfg(target_os = "linux")]
fn cpu_temperature() -> Option<f64> {
    let mut dirs: Vec<_> = std::fs::read_dir("/sys/class/hwmon").ok()?.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for d in dirs {
        let name = read_trim(&d.join("name").to_string_lossy()).unwrap_or_default();
        if !matches!(name.as_str(), "coretemp" | "k10temp" | "zenpower") {
            continue;
        }
        if let Some(v) = read_trim(&d.join("temp1_input").to_string_lossy()).and_then(|v| v.parse::<f64>().ok()) {
            return Some(v / 1000.0);
        }
    }
    None
}

/// AMD GPU through sysfs: (busy %, VRAM used, VRAM total).
#[cfg(target_os = "linux")]
fn amd_gpu() -> Option<(f64, f64, f64)> {
    for card in ["card1", "card0", "card2"] {
        let base = format!("/sys/class/drm/{card}/device");
        let Some(busy) = read_trim(&format!("{base}/gpu_busy_percent")).and_then(|v| v.parse::<f64>().ok()) else { continue };
        let num = |f: &str| read_trim(&format!("{base}/{f}")).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        return Some((busy, num("mem_info_vram_used"), num("mem_info_vram_total")));
    }
    None
}

/// NVIDIA through `nvidia-smi`: (util %, temperature, VRAM used, VRAM total in bytes).
/// `Err` when the tool is missing.
#[cfg(target_os = "linux")]
fn nvidia_smi() -> std::io::Result<Option<(f64, f64, f64, f64)>> {
    let out = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=utilization.gpu,temperature.gpu,memory.used,memory.total", "--format=csv,noheader,nounits"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()?;
    Ok(parse_nvidia(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(any(target_os = "linux", test))]
pub fn parse_nvidia(s: &str) -> Option<(f64, f64, f64, f64)> {
    let v: Vec<f64> = s.lines().next()?.split(',').map(|x| x.trim().parse::<f64>()).collect::<Result<_, _>>().ok()?;
    if v.len() < 4 {
        return None;
    }
    Some((v[0], v[1], v[2] * 1024.0 * 1024.0, v[3] * 1024.0 * 1024.0))
}

// ---------------------------------------------------------------------- other systems

#[cfg(not(target_os = "linux"))]
impl Sampler {
    fn read(&mut self) -> Reading {
        let first = self.sys.is_none();
        let sys = self.sys.get_or_insert_with(sysinfo::System::new);
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let mut out = Reading::default();
        if !first {
            out.cpu = Some(sys.global_cpu_usage() as f64);
        }
        out.mem_total = sys.total_memory() as f64;
        out.mem_used = sys.total_memory().saturating_sub(sys.available_memory()) as f64;
        let comps = sysinfo::Components::new_with_refreshed_list();
        out.cpu_temp = comps
            .list()
            .iter()
            .filter(|c| {
                let l = c.label().to_ascii_lowercase();
                l.contains("cpu") || l.contains("package") || l.contains("tctl") || l.contains("core")
            })
            .find_map(|c| c.temperature())
            .map(|t| t as f64);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_files() {
        let (t, i) = parse_proc_stat("cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 1 2 3").unwrap();
        assert_eq!((t, i), (1000, 850));
        let (total, avail) = parse_meminfo("MemTotal:       16000000 kB\nMemFree: 1 kB\nMemAvailable:    8000000 kB\n").unwrap();
        assert_eq!(total, 16000000.0 * 1024.0);
        assert_eq!(avail, 8000000.0 * 1024.0);
        let n = parse_nvidia("15, 54, 1228, 8192\n").unwrap();
        assert_eq!((n.0, n.1), (15.0, 54.0));
        assert_eq!(n.3, 8192.0 * 1024.0 * 1024.0);
        assert!(parse_nvidia("").is_none());
    }

    #[test]
    fn gigabytes() {
        let g = (1u64 << 30) as f64;
        assert_eq!(gb(5.54 * g), "5.5");
        assert_eq!(gb(15.4 * g), "15");
        assert_eq!(gb(9.96 * g), "10.0");
    }

    #[test]
    fn renders_rows() {
        let mut m = SystemMonitor::new();
        m.apply(&Reading { cpu: Some(40.0), cpu_temp: Some(37.0), mem_used: 5.0e9, mem_total: 16.0e9, gpu: None, gpu_temp: None, vram: None });
        m.apply(&Reading { cpu: Some(60.0), cpu_temp: Some(37.0), mem_used: 5.0e9, mem_total: 16.0e9, gpu: None, gpu_temp: None, vram: None });
        let f = m.frame(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap().and_hms_opt(0, 46, 0).unwrap());
        assert_eq!(f.pixel(0, 0), [0x0a, 0x0e, 0x16]);
        // CPU bar filled at 60%
        assert_eq!(f.pixel(20, 37), [0x5a, 0xc8, 0x78]);
        assert_eq!(m.status(), "CPU 60%  ·  RAM 4.7 ГБ");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn samples_this_machine() {
        let mut s = Sampler::default();
        let first = s.read();
        assert!(first.mem_total > 0.0 && first.mem_used > 0.0);
        assert!(first.cpu.is_none());
        std::thread::sleep(std::time::Duration::from_millis(50));
        let second = s.read();
        assert!(second.cpu.is_none_or(|c| (0.0..=100.0).contains(&c)));
    }
}
