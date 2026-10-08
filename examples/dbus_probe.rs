//! Live checks of the desktop integrations.
//!
//! `cargo run --example dbus_probe -- notify|screensaver|bluez [MAC]|tray|icon OUT.png`
//!
//! - `notify`: monitors Notify calls for 20 s; send one with `notify-send "MiniToo test" "hello"`.
//! - `screensaver`: listens for 30 s; the unique name to target with
//!   `dbus-send --session --dest=:1.N --type=signal /ScreenSaver org.freedesktop.ScreenSaver.ActiveChanged boolean:true`
//!   is logged at start (never call `Lock`).
//! - `bluez`: read-only `GetManagedObjects` poll.
//! - `tray`: shows the tray icon for 8 s, cycling colours.

use minitoo::color::Color;
use minitoo::platform::{bluez, notifications, screensaver, tray};
use std::sync::Arc;
use std::time::Duration;

struct Stderr;

impl log::Log for Stderr {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        if r.target().starts_with("minitoo") {
            eprintln!("[{}] {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

fn main() {
    log::set_logger(&Stderr).unwrap();
    log::set_max_level(log::LevelFilter::Debug);
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let h = rt.handle().clone();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("notify") => {
            let _guard = notifications::spawn_monitor(&h, Arc::new(|n| println!("notification: {n:?}")));
            println!("monitoring for 20 s…");
            std::thread::sleep(Duration::from_secs(20));
        }
        Some("screensaver") => {
            screensaver::spawn_watch(&h, Arc::new(|active| println!("ActiveChanged({active})")));
            println!("listening for 30 s… (unique name is printed by the log line below)");
            std::thread::sleep(Duration::from_secs(30));
        }
        Some("bluez") => {
            let mac = args.get(1).cloned().unwrap_or_else(|| "B1:21:81:05:E2:65".into());
            println!("{mac}: {:?}", rt.block_on(bluez::poll(mac.clone())));
        }
        Some("tray") => {
            let state = |screen: Color, claude: bool, streaming: bool| tray::TrayState {
                screen,
                tooltip: format!("MiniToo Studio\nКолонка: проверка\nРежим: probe\nClaude: {claude}"),
                claude_mode: claude,
                streaming,
            };
            let on = Arc::new(|a| println!("tray action: {a:?}"));
            match tray::Tray::spawn(&h, state(Color::hex(0xd97757), false, false), on) {
                Some(t) => {
                    println!("tray registered");
                    for (i, c) in [0xe04030, 0x5070b0, 0x808088, 0xd97757].into_iter().enumerate() {
                        std::thread::sleep(Duration::from_secs(2));
                        t.update(state(Color::hex(c), i % 2 == 0, i % 2 == 1));
                        println!("updated: #{c:06x}");
                    }
                }
                None => println!("no tray"),
            }
        }
        Some("icon") => {
            let out = args.get(1).map(String::as_str).unwrap_or("icon.png");
            tray::icon_rgba(Color::hex(0xd97757), 64).save(out).unwrap();
            println!("saved {out}");
        }
        _ => eprintln!("usage: dbus_probe notify|screensaver|bluez [MAC]|tray|icon OUT.png"),
    }
}
