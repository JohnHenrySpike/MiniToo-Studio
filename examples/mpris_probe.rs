//! MPRIS probes (read-only on the bus).
//!
//! * `mpris_probe list` — every MPRIS player with its metadata, and the one the mode would pick;
//! * `mpris_probe nowplaying <out.png> [secs]` — runs the «Сейчас играет» mode, saves its frame and
//!   reports the device frames;
//! * `mpris_probe sample <dir>` — renders sample frames (no bus);
//! * `mpris_probe fake-player <art.png>` — serves a fake player (for a private test bus only).

#[cfg(target_os = "linux")]
fn main() {
    linux::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("mpris_probe: MPRIS is Linux-only");
}

#[cfg(target_os = "linux")]
mod linux {
    use minitoo::live::nowplaying::{mpris, NowPlaying, PlayerInfo};
    use minitoo::live::{ModeEvent, ModeHost, ModeSink, Services};
    use minitoo::settings::Settings;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    pub fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        match args.first().map(String::as_str).unwrap_or("list") {
            "list" => list(),
            "nowplaying" => nowplaying(
                args.get(1).map(String::as_str).unwrap_or("nowplaying.png"),
                args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3.0),
            ),
            "sample" => sample(args.get(1).map(String::as_str).unwrap_or(".")),
            "fake-player" => fake_player(args.get(1).cloned().unwrap_or_default()),
            _ => eprintln!("usage: mpris_probe list | nowplaying <out.png> [secs] | sample <dir> | fake-player <art.png>"),
        }
    }

    fn list() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let conn = match mpris::session().await {
                Ok(c) => c,
                Err(e) => return eprintln!("{e}"),
            };
            match mpris::players(&conn).await {
                Err(e) => eprintln!("{e}"),
                Ok(list) => {
                    if list.is_empty() {
                        println!("no MPRIS players on the session bus");
                    }
                    for (name, info) in &list {
                        let id = mpris::identity(&conn, name).await;
                        match info {
                            None => println!("{name} ({id}): no answer"),
                            Some(p) => println!(
                                "{name} ({id}): {} | «{}» — {} | album «{}» | {:.1}/{:.1} s | art {}",
                                p.status,
                                p.title,
                                p.artist,
                                p.album,
                                p.position as f64 / 1e6,
                                p.length as f64 / 1e6,
                                if p.art_url.is_empty() { "-" } else { &p.art_url }
                            ),
                        }
                    }
                    match mpris::choose(list) {
                        Some(p) => println!("chosen: {}", p.service),
                        None => println!("chosen: none («Ничего не играет»)"),
                    }
                }
            }
        });
    }

    fn nowplaying(out: &str, secs: f64) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<ModeEvent>();
        let tx = std::sync::Mutex::new(tx);
        let sink: ModeSink = Arc::new(move |ev| {
            let _ = tx.lock().unwrap().send(ev);
        });
        let services = Services { rt: rt.handle().clone(), http: reqwest::Client::new(), sink };
        let mut settings = Settings::memory();
        let mut host = ModeHost::new(services);
        let mut signals = 0;
        if host.acquire("nowplaying", &mut settings, &[]).device_changed {
            signals += 1;
            let n = host.device_frames("nowplaying", &mut settings, &[]).map(|f| f.0.len()).unwrap_or(0);
            println!(" 0.00s device frames changed → {n} frames");
        }
        let t0 = Instant::now();
        while t0.elapsed().as_secs_f64() < secs {
            if let Ok(ev) = rx.recv_timeout(Duration::from_millis(50)) {
                let fx = host.dispatch(ev, &mut settings, &[]);
                if fx.device_changed {
                    // as if the mode were on the device: fetch the new batch at once
                    signals += 1;
                    let n = host.device_frames("nowplaying", &mut settings, &[]).map(|f| f.0.len()).unwrap_or(0);
                    println!("{:5.2}s device frames changed → {n} frames", t0.elapsed().as_secs_f64());
                }
            }
        }
        let frames = host.device_frames("nowplaying", &mut settings, &[]);
        let slot = host.get("nowplaying").unwrap();
        println!("status: {}", slot.status);
        println!("view: {:?}", slot.mode.view());
        println!("device signals: {signals}, device frames: {}", frames.as_ref().map(|f| f.0.len()).unwrap_or(0));
        if let Some(f) = &slot.frame {
            std::fs::write(out, f.png()).unwrap();
            println!("saved {out}");
        }
        if let Some((frames, _)) = frames {
            if frames.len() > 1 {
                let last = out.replace(".png", "-last.png");
                std::fs::write(&last, frames.last().unwrap().png()).unwrap();
                println!("saved {last}");
            }
        }
        host.release("nowplaying", &mut settings, &[]);
    }

    /// A synthetic cover: diagonal gradient with a circle.
    fn test_cover() -> image::RgbaImage {
        image::RgbaImage::from_fn(400, 400, |x, y| {
            let d = (((x as f32 - 260.0).powi(2) + (y as f32 - 150.0).powi(2)).sqrt() < 90.0) as u8;
            if d == 1 {
                image::Rgba([250, 210, 90, 255])
            } else {
                image::Rgba([(40 + x / 3) as u8, (30 + y / 4) as u8, (160 - x / 4 + y / 8) as u8, 255])
            }
        })
    }

    fn sample(dir: &str) {
        let mut np = NowPlaying::new();
        let info = PlayerInfo {
            service: "org.mpris.MediaPlayer2.test".into(),
            identity: "Test Player".into(),
            title: "Midnight City".into(),
            artist: "M83".into(),
            length: 243_000_000,
            position: 42_000_000,
            status: "Playing".into(),
            ..Default::default()
        };
        np.set_state(info.clone(), Some(test_cover()), true);
        std::fs::write(format!("{dir}/np-art.png"), np.render_at(info.position).png()).unwrap();
        np.set_state(PlayerInfo { status: "Paused".into(), ..info.clone() }, None, false);
        std::fs::write(format!("{dir}/np-noart.png"), np.render_at(info.position).png()).unwrap();
        let _ = test_cover().save(format!("{dir}/cover.png"));
        println!("saved {dir}/np-art.png, np-noart.png, cover.png");
    }

    // ---------------------------------------------------------------------- fake player

    struct Root;

    #[zbus::interface(name = "org.mpris.MediaPlayer2")]
    impl Root {
        #[zbus(property)]
        fn identity(&self) -> String {
            "Test Player".into()
        }
    }

    struct Player {
        start: Instant,
        art: String,
    }

    #[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
    impl Player {
        #[zbus(property)]
        fn playback_status(&self) -> String {
            std::env::var("FAKE_STATUS").unwrap_or_else(|_| "Playing".into())
        }
        #[zbus(property)]
        fn position(&self) -> i64 {
            42_000_000 + self.start.elapsed().as_micros() as i64
        }
        #[zbus(property)]
        fn metadata(&self) -> std::collections::HashMap<String, zbus::zvariant::OwnedValue> {
            use zbus::zvariant::{OwnedValue, Value};
            let mut m = std::collections::HashMap::new();
            let mut put = |k: &str, v: Value| {
                m.insert(k.to_string(), OwnedValue::try_from(v).unwrap());
            };
            put("xesam:title", Value::from(std::env::var("FAKE_TITLE").unwrap_or_else(|_| "Midnight City".into())));
            put("xesam:artist", Value::from(vec!["M83"]));
            put("xesam:album", Value::from("Hurry Up, We're Dreaming"));
            put("mpris:length", Value::from(243_000_000i64));
            if !self.art.is_empty() && std::env::var("FAKE_NO_ART").is_err() {
                put("mpris:artUrl", Value::from(format!("file://{}", self.art)));
            }
            m
        }
        fn play_pause(&self) {}
        fn next(&self) {}
        fn previous(&self) {}
    }

    fn fake_player(art: String) {
        if !art.is_empty() && !std::path::Path::new(&art).exists() {
            test_cover().save(&art).unwrap();
        }
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let _conn = zbus::connection::Builder::session()
                .unwrap()
                .name("org.mpris.MediaPlayer2.minitootest")
                .unwrap()
                .serve_at(mpris::PATH, Root)
                .unwrap()
                .serve_at(mpris::PATH, Player { start: Instant::now(), art })
                .unwrap()
                .build()
                .await
                .unwrap();
            println!("fake player up");
            std::future::pending::<()>().await;
        });
    }
}
