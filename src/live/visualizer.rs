//! Визуализатор звука (`visualizer`, §8.8): spectrum of what the computer plays.
//!
//! Audio is captured on its own thread (Linux: a PipeWire capture stream on the default sink
//! monitor; Windows: WASAPI loopback of the default output; macOS: the default input, both via
//! cpal) into a mono ring buffer. Every 60 ms the newest 2048 samples are analysed and drawn.

use super::{LiveMode, ModeCommand, ModeCx, ModeMsg, ModeView, VisualizerView};
use crate::canvas::{r, Align, Canvas};
use crate::color::Color;
use crate::fonts::FontSpec;
use parking_lot::Mutex;
use rustfft::num_complex::Complex;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const FFT_SIZE: usize = 2048;
pub const BANDS: usize = 20;
pub const RATE: f64 = 48000.0;
const FRAME_MS: u64 = 60;
const TIMER_RENDER: u64 = 1;

// ---------------------------------------------------------------------- ring buffer

/// Mono samples, the newest `FFT_SIZE` of them.
pub struct Ring {
    buf: Vec<f32>,
    pos: usize,
    /// mono samples received so far
    pub received: u64,
}

impl Default for Ring {
    fn default() -> Self {
        Ring { buf: vec![0.0; FFT_SIZE], pos: 0, received: 0 }
    }
}

impl Ring {
    /// Interleaved frames of `channels` samples, mixed down to mono.
    pub fn push_interleaved(&mut self, samples: impl IntoIterator<Item = f32>, channels: usize) {
        let ch = channels.max(1);
        let (mut acc, mut n) = (0.0f32, 0usize);
        for s in samples {
            acc += s;
            n += 1;
            if n == ch {
                self.buf[self.pos] = acc / ch as f32;
                self.pos = (self.pos + 1) % self.buf.len();
                self.received += 1;
                acc = 0.0;
                n = 0;
            }
        }
    }

    /// Oldest first.
    pub fn snapshot(&self) -> Vec<f32> {
        (0..self.buf.len()).map(|i| self.buf[(self.pos + i) % self.buf.len()]).collect()
    }
}

pub type SharedRing = Arc<Mutex<Ring>>;

// ---------------------------------------------------------------------- analysis

/// FFT bins `[k0, k1)` of band `b` (log-spaced, 40 Hz … 16 kHz).
pub fn band_bins(b: usize) -> (usize, usize) {
    let f0 = 40.0 * 400f64.powf(b as f64 / BANDS as f64);
    let f1 = 40.0 * 400f64.powf((b + 1) as f64 / BANDS as f64);
    let k0 = ((f0 * FFT_SIZE as f64 / RATE) as usize).max(1);
    let k1 = ((f1 * FFT_SIZE as f64 / RATE) as usize).max(k0 + 1);
    (k0, k1)
}

/// Raw FFT magnitude → bar level: `clamp((dB + 10) / 50, 0, 1.5)`.
pub fn db_level(magnitude: f64) -> f64 {
    let db = 20.0 * (magnitude + 1e-9).log10();
    ((db + 10.0) / 50.0).clamp(0.0, 1.5)
}

pub struct Analyzer {
    fft: Arc<dyn rustfft::Fft<f64>>,
    window: Vec<f64>,
    pub levels: [f64; BANDS],
    pub peaks: [f64; BANDS],
    pub gain: f64,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer {
    pub fn new() -> Self {
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / (FFT_SIZE - 1) as f64).cos()))
            .collect();
        Analyzer {
            fft: rustfft::FftPlanner::new().plan_fft_forward(FFT_SIZE),
            window,
            levels: [0.0; BANDS],
            peaks: [0.0; BANDS],
            gain: 1.0,
        }
    }

    /// Band levels (before gain) of `samples` (oldest first, `FFT_SIZE` long).
    pub fn bands(&self, samples: &[f32]) -> [f64; BANDS] {
        let mut a: Vec<Complex<f64>> =
            (0..FFT_SIZE).map(|i| Complex::new(samples.get(i).copied().unwrap_or(0.0) as f64 * self.window[i], 0.0)).collect();
        self.fft.process(&mut a);
        let mut out = [0.0; BANDS];
        for (b, o) in out.iter_mut().enumerate() {
            let (k0, k1) = band_bins(b);
            let peak = (k0..k1.min(FFT_SIZE / 2)).map(|k| a[k].norm()).fold(0.0, f64::max);
            *o = db_level(peak);
        }
        out
    }

    /// One analysis step; returns true if the sound was loud enough to count as "not silence".
    pub fn step(&mut self, samples: &[f32]) -> bool {
        let bands = self.bands(samples);
        let loudest = bands.iter().copied().fold(0.0, f64::max);
        let loud = loudest > 0.05;
        if loud {
            // slow automatic gain so quiet and loud music both fill the screen
            self.gain = (self.gain * 0.97 + (0.9 / loudest) * 0.03).clamp(0.4, 3.0);
        }
        for b in 0..BANDS {
            let v = (bands[b] * self.gain).clamp(0.0, 1.0);
            // fast attack, slow decay
            self.levels[b] = if v > self.levels[b] { v } else { self.levels[b] * 0.75 + v * 0.25 };
            self.peaks[b] = (self.peaks[b] - 0.02).max(self.levels[b]);
        }
        loud
    }
}

// ---------------------------------------------------------------------- drawing

/// The frame for the given levels; `quiet` shows «тишина…».
pub fn draw(style: i64, levels: &[f64; BANDS], peaks: &[f64; BANDS], quiet: bool) -> Canvas {
    let mut c = Canvas::device();
    c.fill(Color::rgb(8, 8, 14));
    if quiet {
        c.text(r(0.0, 50.0, 160.0, 24.0), Align::CENTER, "тишина…", FontSpec::bold(12.0), Color::rgb(90, 90, 120));
    } else if style == 0 {
        // segmented bars with peak caps
        c.aa = false;
        let bw = 7.4f32;
        for b in 0..BANDS {
            let h = (levels[b] * 108.0) as i32;
            let x = (6.0 + b as f32 * bw).round(); // QPainter rounds aliased rectangles
            let mut k = 0;
            while k < h {
                let t = k as f64 / 108.0;
                let col = Color::rgb((80.0 + 175.0 * t) as u8, (200.0 - 120.0 * t) as u8, (255.0 - 100.0 * t) as u8);
                c.fill_rect(x, (122 - k - 3) as f32, 5.0, 3.0, col);
                k += 5;
            }
            let py = 122 - (peaks[b] * 108.0) as i32 - 4;
            c.fill_rect(x, py as f32, 5.0, 2.0, Color::WHITE);
        }
    } else {
        // mirrored smooth bars from the middle
        let bw = 8.0f32;
        for b in 0..BANDS {
            let h = (levels[b] * 58.0).max(1.0) as f32;
            let col = Color::from_hsv(((0.55 + b as f64 / 40.0) % 1.0) as f32, 0.7, 1.0);
            if let Some(path) = rounded_rect_xy(1.0 + b as f32 * bw, 64.0 - h, 6.0, 2.0 * h, 3.0) {
                c.fill_path(&path, col);
            }
        }
    }
    c
}

/// A rounded rectangle whose corner radii are clamped per axis like `QPainter::drawRoundedRect`
/// (a short bar gets elliptical corners, `r` wide and `h/2` high).
fn rounded_rect_xy(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<tiny_skia::Path> {
    let rx = radius.min(w / 2.0).max(0.0);
    let ry = radius.min(h / 2.0).max(0.0);
    let (kx, ky) = (0.552_284_8 * rx, 0.552_284_8 * ry);
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x + rx, y);
    pb.line_to(x + w - rx, y);
    pb.cubic_to(x + w - rx + kx, y, x + w, y + ry - ky, x + w, y + ry);
    pb.line_to(x + w, y + h - ry);
    pb.cubic_to(x + w, y + h - ry + ky, x + w - rx + kx, y + h, x + w - rx, y + h);
    pb.line_to(x + rx, y + h);
    pb.cubic_to(x + rx - kx, y + h, x, y + h - ry + ky, x, y + h - ry);
    pb.line_to(x, y + ry);
    pb.cubic_to(x, y + ry - ky, x + rx - kx, y, x + rx, y);
    pb.close();
    pb.finish()
}

// ---------------------------------------------------------------------- the mode

/// Posted by the capture thread.
enum AudioMsg {
    Error(String),
}

pub struct Visualizer {
    style: i64,
    analyzer: Analyzer,
    ring: SharedRing,
    capture: Option<capture::AudioCapture>,
    last_sound: Instant,
    error: Option<String>,
}

impl Default for Visualizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualizer {
    pub fn new() -> Self {
        Visualizer {
            style: 0,
            analyzer: Analyzer::new(),
            ring: Arc::default(),
            capture: None,
            last_sound: Instant::now(),
            error: None,
        }
    }

    fn fail(&mut self, cx: &mut ModeCx, e: String) {
        cx.log(format!("visualizer: {e}"));
        cx.set_status(e.clone());
        self.error = Some(e);
        self.capture = None;
    }
}

impl LiveMode for Visualizer {
    fn id(&self) -> &'static str {
        "visualizer"
    }
    fn title(&self) -> &'static str {
        "Визуализатор звука"
    }
    fn subtitle(&self) -> &'static str {
        "спектр того, что играет на компьютере"
    }
    fn icon(&self) -> &'static str {
        "wave"
    }
    fn streaming(&self) -> bool {
        true
    }

    fn start(&mut self, cx: &mut ModeCx) {
        self.style = cx.settings.int_in("visualizer/style", 0, 0, 1);
        self.analyzer = Analyzer::new();
        *self.ring.lock() = Ring::default();
        self.error = None;
        self.last_sound = Instant::now();
        let sender = cx.sender();
        let on_error = Box::new(move |e: String| sender.send(Box::new(AudioMsg::Error(e))));
        match capture::AudioCapture::start(self.ring.clone(), on_error) {
            Ok(c) => {
                self.capture = Some(c);
                cx.set_status("слушаю системный звук");
            }
            Err(e) => self.fail(cx, e),
        }
        cx.timer(Duration::from_millis(FRAME_MS), TIMER_RENDER);
    }

    fn stop(&mut self, _cx: &mut ModeCx) {
        self.capture = None; // stops the capture thread
    }

    fn render(&mut self, cx: &mut ModeCx) {
        let samples = self.ring.lock().snapshot();
        if self.analyzer.step(&samples) {
            self.last_sound = Instant::now();
        }
        let quiet = self.last_sound.elapsed() > Duration::from_secs(3);
        let c = draw(self.style, &self.analyzer.levels, &self.analyzer.peaks, quiet);
        cx.publish(c.to_frame());
    }

    fn on_timer(&mut self, cx: &mut ModeCx, token: u64) {
        if token == TIMER_RENDER {
            self.render(cx);
            cx.timer(Duration::from_millis(FRAME_MS), TIMER_RENDER);
        }
    }

    fn on_message(&mut self, cx: &mut ModeCx, msg: ModeMsg) {
        if let Ok(m) = msg.downcast::<AudioMsg>() {
            match *m {
                AudioMsg::Error(e) => self.fail(cx, e),
            }
        }
    }

    fn command(&mut self, cx: &mut ModeCx, cmd: ModeCommand) {
        if let ModeCommand::VisualizerStyle(s) = cmd {
            self.style = s.clamp(0, 1);
            cx.settings.set_int("visualizer/style", self.style);
        }
    }

    fn view(&self) -> ModeView {
        ModeView::Visualizer(VisualizerView { style: self.style, error: self.error.clone() })
    }
}

// ---------------------------------------------------------------------- audio capture

pub type ErrorFn = Box<dyn Fn(String) + Send + Sync>;

#[cfg(target_os = "linux")]
pub mod capture {
    //! PipeWire capture of the default sink monitor.

    use super::{ErrorFn, SharedRing};
    use crate::platform::pipewire_util::{self, PwThread};
    use pipewire as pw;
    use pw::spa;
    use std::sync::Arc;

    pub struct AudioCapture {
        _thread: PwThread,
    }

    struct Data {
        ring: SharedRing,
        channels: usize,
        on_error: Arc<ErrorFn>,
    }

    impl AudioCapture {
        /// Starts the capture thread. Errors after the start go to `on_error` (any thread).
        pub fn start(ring: SharedRing, on_error: ErrorFn) -> Result<AudioCapture, String> {
            let on_error = Arc::new(on_error);
            let err2 = on_error.clone();
            let thread = PwThread::spawn(
                "minitoo-audio",
                move |ml| setup(ml, ring, on_error),
                move |e| err2(e),
            )
            .map_err(|_| "нет подключения к PipeWire".to_string())?;
            Ok(AudioCapture { _thread: thread })
        }
    }

    fn setup(ml: &pw::main_loop::MainLoopRc, ring: SharedRing, on_error: Arc<ErrorFn>) -> Result<pipewire_util::Guard, String> {
        let no_pw = |_| "нет подключения к PipeWire".to_string();
        let context = pw::context::ContextRc::new(ml, None).map_err(no_pw)?;
        let core = context.connect_rc(None).map_err(no_pw)?;
        let stream = pw::stream::StreamRc::new(
            core,
            "minitoo-visualizer",
            pw::properties::properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Music",
                *pw::keys::STREAM_CAPTURE_SINK => "true",
                *pw::keys::NODE_NAME => "minitoo-visualizer",
                *pw::keys::APP_NAME => "MiniToo Studio",
            },
        )
        .map_err(|e| format!("PipeWire: {e}"))?;

        let listener = stream
            .add_local_listener_with_user_data(Data { ring, channels: 2, on_error })
            .state_changed(|_, d, _old, new| {
                if let pw::stream::StreamState::Error(e) = new {
                    (d.on_error)(format!("ошибка захвата звука: {e}"));
                }
            })
            .param_changed(|_, d, id, param| {
                let Some(param) = param else { return };
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let mut info = spa::param::audio::AudioInfoRaw::new();
                if info.parse(param).is_ok() && info.channels() > 0 {
                    d.channels = info.channels() as usize;
                }
            })
            .process(|stream, d| {
                let Some(mut buf) = stream.dequeue_buffer() else { return };
                let datas = buf.datas_mut();
                let Some(first) = datas.first_mut() else { return };
                let (offset, size) = (first.chunk().offset() as usize, first.chunk().size() as usize);
                let Some(bytes) = first.data() else { return };
                let end = (offset + size).min(bytes.len());
                let Some(src) = bytes.get(offset.min(end)..end) else { return };
                let samples = src.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                d.ring.lock().push_interleaved(samples, d.channels);
            })
            .register()
            .map_err(|e| format!("PipeWire: {e}"))?;

        let mut info = spa::param::audio::AudioInfoRaw::new();
        info.set_format(spa::param::audio::AudioFormat::F32LE);
        info.set_rate(super::RATE as u32);
        info.set_channels(2);
        let bytes = pipewire_util::pod_bytes(spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: info.into(),
        }));
        let pod = spa::pod::Pod::from_bytes(&bytes).ok_or("PipeWire: формат")?;
        stream
            .connect(
                spa::utils::Direction::Input,
                None,
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut [pod],
            )
            .map_err(|e| format!("pw_stream_connect: {e}"))?;
        Ok(Box::new((listener, stream, context)))
    }
}

#[cfg(not(target_os = "linux"))]
pub mod capture {
    //! cpal capture: WASAPI loopback of the default output on Windows, the default input
    //! elsewhere.

    use super::{ErrorFn, SharedRing};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::Arc;
    use std::sync::mpsc;

    pub struct AudioCapture {
        quit: Option<mpsc::Sender<()>>,
    }

    impl Drop for AudioCapture {
        fn drop(&mut self) {
            // the thread drops the stream and ends
            self.quit.take();
        }
    }

    fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, ring: SharedRing, on_error: Arc<ErrorFn>) -> Result<cpal::Stream, String>
    where
        T: cpal::SizedSample,
        f32: cpal::FromSample<T>,
    {
        let channels = config.channels as usize;
        device
            .build_input_stream(
                config,
                move |data: &[T], _: &cpal::InputCallbackInfo| {
                    ring.lock().push_interleaved(data.iter().map(|s| <f32 as cpal::FromSample<T>>::from_sample_(*s)), channels);
                },
                move |e| on_error(format!("ошибка захвата звука: {e}")),
                None,
            )
            .map_err(|e| format!("ошибка захвата звука: {e}"))
    }

    fn open(ring: SharedRing, on_error: Arc<ErrorFn>) -> Result<cpal::Stream, String> {
        let host = cpal::default_host();
        #[cfg(windows)]
        let (device, config) = {
            let d = host.default_output_device().ok_or("нет устройства вывода звука")?;
            let c = d.default_output_config().map_err(|e| format!("ошибка захвата звука: {e}"))?;
            (d, c)
        };
        #[cfg(not(windows))]
        let (device, config) = {
            let d = host.default_input_device().ok_or("нет устройства записи звука")?;
            let c = d.default_input_config().map_err(|e| format!("ошибка захвата звука: {e}"))?;
            (d, c)
        };
        let fmt = config.sample_format();
        let cfg: cpal::StreamConfig = config.into();
        let stream = match fmt {
            cpal::SampleFormat::F32 => build::<f32>(&device, cfg, ring, on_error)?,
            cpal::SampleFormat::I16 => build::<i16>(&device, cfg, ring, on_error)?,
            cpal::SampleFormat::I32 => build::<i32>(&device, cfg, ring, on_error)?,
            cpal::SampleFormat::U16 => build::<u16>(&device, cfg, ring, on_error)?,
            cpal::SampleFormat::F64 => build::<f64>(&device, cfg, ring, on_error)?,
            other => return Err(format!("неподдерживаемый формат звука: {other}")),
        };
        stream.play().map_err(|e| format!("ошибка захвата звука: {e}"))?;
        Ok(stream)
    }

    impl AudioCapture {
        pub fn start(ring: SharedRing, on_error: ErrorFn) -> Result<AudioCapture, String> {
            let on_error = Arc::new(on_error);
            let (quit, rx) = mpsc::channel::<()>();
            let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
            // cpal streams are not Send everywhere: the stream lives on its own thread
            std::thread::Builder::new()
                .name("minitoo-audio".into())
                .spawn(move || match open(ring, on_error) {
                    Ok(stream) => {
                        let _ = ready_tx.send(Ok(()));
                        let _ = rx.recv(); // until the sender is dropped
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                })
                .map_err(|e| e.to_string())?;
            match ready_rx.recv_timeout(std::time::Duration::from_secs(3)) {
                Ok(Ok(())) | Err(_) => Ok(AudioCapture { quit: Some(quit) }),
                Ok(Err(e)) => Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_edges_cover_40hz_to_16khz() {
        let (k0, _) = band_bins(0);
        let (_, k_last) = band_bins(BANDS - 1);
        assert_eq!(k0, 1); // 40 Hz → bin 1.7 → 1
        assert_eq!(k_last, (16000.0 * FFT_SIZE as f64 / RATE) as usize); // 682
        for b in 0..BANDS {
            let (a, z) = band_bins(b);
            assert!(z > a, "band {b} is empty");
            if b > 0 {
                assert!(band_bins(b - 1).1 <= z);
            }
        }
    }

    #[test]
    fn db_mapping() {
        assert_eq!(db_level(0.0), 0.0);
        assert!((db_level(10f64.powf(-10.0 / 20.0)) - 0.0).abs() < 1e-6); // -10 dB → 0
        assert!((db_level(10f64.powf(15.0 / 20.0)) - 0.5).abs() < 1e-6); // 15 dB → 0.5
        assert!((db_level(1e6) - 1.5).abs() < 1e-9); // clamped
    }

    #[test]
    fn sine_lights_its_band_and_decays() {
        let mut a = Analyzer::new();
        let freq = 1000.0;
        let s: Vec<f32> = (0..FFT_SIZE).map(|i| (0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / RATE).sin()) as f32).collect();
        assert!(a.step(&s));
        let loudest = (0..BANDS).max_by(|&x, &y| a.levels[x].total_cmp(&a.levels[y])).unwrap();
        let (k0, k1) = band_bins(loudest);
        let bin = freq * FFT_SIZE as f64 / RATE;
        assert!((k0 as f64) <= bin + 1.0 && bin <= k1 as f64 + 1.0, "band {loudest} = {k0}..{k1}, bin {bin}");
        let before = a.levels[loudest];
        let peak = a.peaks[loudest];
        let silent = vec![0.0f32; FFT_SIZE];
        assert!(!a.step(&silent));
        assert!((a.levels[loudest] - before * 0.75).abs() < 1e-9);
        assert!((a.peaks[loudest] - (peak - 0.02).max(a.levels[loudest])).abs() < 1e-9);
    }

    #[test]
    fn ring_mixes_to_mono() {
        let mut r = Ring::default();
        r.push_interleaved([1.0, 0.0, 0.5, 0.5], 2);
        let s = r.snapshot();
        assert_eq!(&s[FFT_SIZE - 2..], &[0.5, 0.5]);
    }

    #[test]
    fn draws_both_styles() {
        let mut lv = [0.0; BANDS];
        for (i, v) in lv.iter_mut().enumerate() {
            *v = i as f64 / BANDS as f64;
        }
        let f0 = draw(0, &lv, &lv, false).to_frame();
        let f1 = draw(1, &lv, &lv, false).to_frame();
        let fq = draw(0, &lv, &lv, true).to_frame();
        assert_ne!(f0, f1);
        assert_eq!(f0.pixel(0, 0), [8, 8, 14]);
        // a peak cap of the loudest band is white
        let x = (6.0f32 + 19.0 * 7.4).round() as usize + 1;
        let y = 122 - (lv[19] * 108.0) as usize - 4;
        assert_eq!(f0.pixel(x, y), [255, 255, 255]);
        assert!(fq.rgb().iter().any(|&b| b > 60));
    }
}
