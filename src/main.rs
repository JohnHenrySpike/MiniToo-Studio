//! MiniToo Studio: command line (§14.2), single instance, window / tray lifecycle.

mod window;

use minitoo::api::{Command, CoreHandle, Fit};
use minitoo::app::{Controller, DEFAULT_MAC, DEFAULT_PORT, StartOptions};
use minitoo::settings::Settings;
use minitoo::tr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn help() -> &'static str {
    tr!("cli.help")
}

#[derive(Default)]
struct Args {
    hidden: bool,
    headless: bool,
    send: Option<PathBuf>,
    fit: Option<String>,
    mode: Option<String>,
    state: Option<String>,
    status: bool,
    no_device: bool,
    image: Option<PathBuf>,
    debug: bool,
    screenshot: Option<PathBuf>,
    export_faces: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| tr!("cli.needs_value", name = name));
        match arg.as_str() {
            "--hidden" => a.hidden = true,
            "--headless" => a.headless = true,
            "--send" => a.send = Some(PathBuf::from(value("--send")?)),
            "--fit" => a.fit = Some(value("--fit")?),
            "--mode" => a.mode = Some(value("--mode")?),
            "--state" => a.state = Some(value("--state")?),
            "--status" => a.status = true,
            "--no-device" => a.no_device = true,
            "--image" => a.image = Some(PathBuf::from(value("--image")?)),
            "--debug" => a.debug = true,
            "--screenshot" => a.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--export-faces" => a.export_faces = Some(PathBuf::from(value("--export-faces")?)),
            "-h" | "--help" => {
                print!("{}", help());
                std::process::exit(0);
            }
            other => return Err(tr!("cli.unknown_arg", arg = other)),
        }
    }
    Ok(a)
}

/// stderr logger for the `log` macros used by the platform modules.
struct StderrLog;

impl log::Log for StderrLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info && m.target().starts_with("minitoo")
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("[minitoo] {}", r.args());
        }
    }
    fn flush(&self) {}
}

fn post(port: u16, path: &str, body: &[u8]) -> Option<(u16, Vec<u8>)> {
    minitoo::http::request(port, "POST", path, body, Duration::from_secs(5))
}

fn main() {
    let had_config = minitoo::settings::config_path().exists();
    let mut settings = Settings::load();
    minitoo::i18n::init(&mut settings, had_config);
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n\n{}", help());
            std::process::exit(2);
        }
    };
    static LOGGER: StderrLog = StderrLog;
    let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Info));
    let port = settings.int_in("claude/port", DEFAULT_PORT as i64, 1024, 65535) as u16;

    if let Some(dir) = &args.export_faces {
        match minitoo::faces::export_faces(dir) {
            Ok(()) => println!("{}", tr!("cli.scenes_written", dir = dir.display())),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // commands for the running instance
    if args.status {
        match minitoo::http::request(port, "GET", "/status", b"", Duration::from_secs(5)) {
            Some((_, body)) => println!("{}", String::from_utf8_lossy(&body)),
            None => {
                eprintln!("{}", tr!("cli.not_running", port = port));
                std::process::exit(1);
            }
        }
        return;
    }
    if let Some(mode) = &args.mode {
        if post(port, &format!("/mode/{mode}"), b"").is_none() {
            eprintln!("{}", tr!("cli.not_running", port = port));
            std::process::exit(1);
        }
        return;
    }
    if let Some(state) = &args.state {
        match post(port, &format!("/state/{state}"), b"") {
            Some((200, _)) => {}
            Some((_, body)) => {
                eprintln!("{}", String::from_utf8_lossy(&body));
                std::process::exit(1);
            }
            None => {
                eprintln!("{}", tr!("cli.not_running", port = port));
                std::process::exit(1);
            }
        }
        return;
    }
    if let Some(file) = &args.send {
        let abs = std::fs::canonicalize(file).unwrap_or(file.clone());
        let fit = args.fit.clone().unwrap_or_else(|| "crop".into());
        let body = serde_json::to_vec(&serde_json::json!({ "path": abs.to_string_lossy(), "fit": fit })).unwrap_or_default();
        if post(port, "/show", &body).is_some() {
            return;
        }
        std::process::exit(send_direct(&settings, &abs, Fit::parse(&fit)));
    }

    // single instance
    if args.screenshot.is_none() && post(port, "/activate", b"").is_some() {
        println!("MiniToo Studio is already running");
        return;
    }

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("minitoo").build().expect("tokio runtime");
    let headless = args.headless;
    let opts = StartOptions {
        connect: !args.no_device && args.screenshot.is_none(),
        image: args.image.clone(),
        debug: args.debug,
        with_tray: !headless && args.screenshot.is_none(),
        screenshot: args.screenshot.is_some(),
    };
    let start_hidden = args.hidden || settings.bool("ui/startHidden", false);
    let (controller, core, rx) = Controller::new(rt.handle().clone(), settings, opts);
    let core_task = rt.spawn(controller.run(rx));

    if headless {
        wait_until(&core, |s, _| s.quit);
    } else {
        run_windows(&core, &args, start_hidden);
    }
    core.send(Command::Quit);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !core_task.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    rt.shutdown_timeout(Duration::from_secs(1));
}

/// Polls the snapshot until `done(snapshot, serial_at_start)` holds.
fn wait_until(core: &CoreHandle, done: impl Fn(&minitoo::api::Snapshot, u64) -> bool) {
    let start = core.snapshot().show_window_serial;
    loop {
        let s = core.snapshot();
        if s.quit || done(&s, start) {
            return;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// The window comes and goes (close to tray); the process ends on "Выход" / Quit.
fn run_windows(core: &CoreHandle, args: &Args, start_hidden: bool) {
    let opts = window::Options { debug: args.debug, screenshot_dir: args.screenshot.clone() };
    let mut hidden = start_hidden && args.screenshot.is_none();
    loop {
        if hidden {
            wait_until(core, |s, start| s.show_window_serial > start);
            if core.snapshot().quit {
                return;
            }
        }
        match window::run(core.clone(), &opts) {
            window::Exit::Quit => return,
            window::Exit::Hidden => hidden = true,
        }
        if args.screenshot.is_some() {
            return;
        }
    }
}

/// `--send` without a running app: our own worker, no keepalive; waits up to 60 s.
fn send_direct(settings: &Settings, path: &std::path::Path, fit: Fit) -> i32 {
    use minitoo::worker::{DeviceWorker, MediaJob, WorkerConfig, WorkerEvent};
    let anim = match minitoo::media::load(path, minitoo::media::MAX_LOAD_FRAMES) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            return 1;
        }
    };
    let crop = minitoo::media::default_crop(anim.width, anim.height);
    let pixel = minitoo::media::auto_pixel_art(anim.width, anim.height);
    let frames = minitoo::media::render_all(&anim, crop, fit, pixel);
    let (frames, speed) = minitoo::media::decimate(frames, anim.average_delay());
    let (tx, rx) = std::sync::mpsc::channel::<WorkerEvent>();
    let tx = std::sync::Mutex::new(tx);
    let worker = DeviceWorker::new(
        WorkerConfig {
            address: minitoo::transport::normalize_mac(&settings.string("device/mac", DEFAULT_MAC)),
            channel: settings.int_in("device/channel", 1, 1, 30) as u8,
            chunk_delay_ms: settings.int_in("device/chunkDelay", 2, 0, 60) as u64,
            keepalive: 0,
        },
        Arc::new(move |e| {
            let _ = tx.lock().map(|t| t.send(e));
        }),
    );
    worker.start();
    let id = worker.submit_media(MediaJob {
        id: 0,
        frames,
        speed,
        level: settings.int_in("device/zstdLevel", 19, 1, 22) as i32,
        depth: minitoo::protocol::ColorDepth::Full,
        streaming: false,
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(WorkerEvent::MediaSent { id: got, bytes, frames, ms, ok }) if got == id => {
                worker.shutdown();
                if ok {
                    println!("sent: {bytes} bytes, {frames} frames, {ms} ms");
                    return 0;
                }
                eprintln!("{}", tr!("cli.not_sent"));
                return 1;
            }
            Ok(WorkerEvent::Log(l)) => eprintln!("{l}"),
            _ => {}
        }
    }
    worker.shutdown();
    eprintln!("{}", tr!("cli.timeout"));
    1
}
