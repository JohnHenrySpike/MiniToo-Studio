//! Capture probes (read-only).
//!
//! * `capture_probe audio [secs]` — system-sound capture of the visualizer, prints band levels;
//! * `capture_probe visualizer <out.png> [secs] [style]` — runs the visualizer mode, saves its frame;
//! * `capture_probe screen [secs]` — one screen-capture session (shows the portal picker!);
//! * `capture_probe portal-check` — the portal steps before `Start` (no dialog), then closes.

use minitoo::live::visualizer::{self, Analyzer, BANDS};
use minitoo::live::{ModeEvent, ModeHost, ModeSink, Services};
use minitoo::settings::Settings;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("audio");
    match cmd {
        "audio" => audio(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(3.0)),
        "visualizer" => run_visualizer(
            args.get(1).map(String::as_str).unwrap_or("visualizer.png"),
            args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.0),
            args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0),
        ),
        "screen" => screen(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(30.0)),
        #[cfg(target_os = "linux")]
        "portal-check" => portal_check(),
        _ => eprintln!("usage: capture_probe audio [secs] | visualizer <out.png> [secs] [style] | screen [secs]"),
    }
}

fn audio(secs: f64) {
    let ring: visualizer::SharedRing = Arc::default();
    let cap = visualizer::capture::AudioCapture::start(ring.clone(), Box::new(|e| eprintln!("capture error: {e}")));
    let _cap = match cap {
        Ok(c) => c,
        Err(e) => return eprintln!("capture failed: {e}"),
    };
    let mut an = Analyzer::new();
    let t0 = Instant::now();
    let mut last_print = Instant::now();
    let mut loud_frames = 0;
    let mut frames = 0;
    while t0.elapsed().as_secs_f64() < secs {
        std::thread::sleep(Duration::from_millis(60));
        let (s, received) = {
            let r = ring.lock();
            (r.snapshot(), r.received)
        };
        let rms = (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        frames += 1;
        if an.step(&s) {
            loud_frames += 1;
        }
        if last_print.elapsed() > Duration::from_millis(400) {
            last_print = Instant::now();
            let bars: String = an.levels.iter().map(|v| [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][(v * 8.0).round() as usize]).collect();
            println!("{:5.2}s {received:7} samples, rms {:.4} gain {:.2} |{bars}|", t0.elapsed().as_secs_f64(), rms, an.gain);
        }
    }
    println!("{frames} analysis frames, {loud_frames} above the silence threshold, {BANDS} bands");
}

/// A minimal controller loop around one mode.
pub fn run_mode(id: &str, secs: f64, settings: &mut Settings) -> (Option<minitoo::frame::Frame>, String, usize, usize) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<ModeEvent>();
    let tx = std::sync::Mutex::new(tx);
    let sink: ModeSink = Arc::new(move |ev| {
        let _ = tx.lock().unwrap().send(ev);
    });
    let services = Services { rt: rt.handle().clone(), http: reqwest::Client::new(), sink };
    let mut host = ModeHost::new(services);
    let mut device_signals = host.acquire(id, settings, &[]).device_changed as usize;
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(50)) {
            device_signals += host.dispatch(ev, settings, &[]).device_changed as usize;
        }
    }
    let frames = host.device_frames(id, settings, &[]).map(|(f, _)| f.len()).unwrap_or(0);
    let slot = host.get(id).unwrap();
    let out = (slot.frame.clone(), slot.status.clone(), device_signals, frames);
    host.release(id, settings, &[]);
    out
}

fn run_visualizer(out: &str, secs: f64, style: i64) {
    let mut settings = Settings::memory();
    settings.set_int("visualizer/style", style);
    let (frame, status, _, _) = run_mode("visualizer", secs, &mut settings);
    println!("status: {status}");
    if let Some(f) = frame {
        std::fs::write(out, f.png()).unwrap();
        println!("saved {out}");
    }
}

fn screen(secs: f64) {
    use minitoo::platform::capture::{stale_portal_units, Capture, CaptureEvent};
    println!("stale portal units: {:?}", stale_portal_units());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let cap = Capture::start(
        rt.handle(),
        None,
        Arc::new(|ev: CaptureEvent| match ev {
            CaptureEvent::Started { width, height, restore_token } => {
                println!("started {width}×{height}, restore token: {}", if restore_token.is_some() { "yes" } else { "no" })
            }
            other => println!("{other:?}"),
        }),
    );
    let t0 = Instant::now();
    let mut last = 0;
    while t0.elapsed().as_secs_f64() < secs {
        std::thread::sleep(Duration::from_millis(500));
        if let Some((n, img)) = cap.latest() {
            if n != last {
                println!("{:5.1}s frame #{n} {}×{}", t0.elapsed().as_secs_f64(), img.width(), img.height());
                if last == 0 {
                    let _ = img.save(std::env::temp_dir().join("minitoo-capture-probe.png"));
                }
                last = n;
            }
        }
    }
    cap.stop();
    std::thread::sleep(Duration::from_millis(300));
}

#[cfg(target_os = "linux")]
fn portal_check() {
    use ashpd::desktop::screencast::{Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::PersistMode;
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let proxy = match Screencast::new().await {
            Ok(p) => p,
            Err(e) => return println!("ScreenCast: {e}"),
        };
        println!("ScreenCast version {}", proxy.version());
        println!("cursor modes: {:?}", proxy.available_cursor_modes().await);
        println!("source types: {:?}", proxy.available_source_types().await);
        let session = match proxy.create_session(Default::default()).await {
            Ok(s) => s,
            Err(e) => return println!("CreateSession: {e}"),
        };
        println!("CreateSession: ok {session:?}");
        let opts = SelectSourcesOptions::default()
            .set_sources(SourceType::Monitor | SourceType::Window)
            .set_multiple(false)
            .set_persist_mode(PersistMode::ExplicitlyRevoked);
        match proxy.select_sources(&session, opts).await.and_then(|r| r.response()) {
            Ok(()) => println!("SelectSources: ok"),
            Err(e) => println!("SelectSources: {e}"),
        }
        println!("Session.Close: {:?}", session.close().await);
    });
}
