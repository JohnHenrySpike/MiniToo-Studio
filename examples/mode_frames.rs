//! Renders the live-mode screens and notification cards to PNG files for visual comparison with
//! the Qt version: `cargo run --example mode_frames -- <out-dir> [language] [24|12]`.

use chrono::{NaiveDate, NaiveDateTime};
use minitoo::claude::{ClaudeState, Session};
use minitoo::frame::Frame;
use minitoo::live::claudestats::{self, Totals};
use minitoo::live::clock::{Clock, Weather};
use minitoo::live::github::{Github, Run};
use minitoo::live::pomodoro::Pomodoro;
use minitoo::live::sysmon::{Reading, SystemMonitor};
use std::collections::HashMap;
use std::path::Path;

fn at(h: u32, m: u32, s: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 10, 9).unwrap().and_hms_opt(h, m, s).unwrap()
}

fn save(dir: &Path, name: &str, f: &Frame) {
    let p = dir.join(format!("{name}.png"));
    std::fs::write(&p, f.png()).unwrap();
    println!("{}", p.display());
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "mode_frames".into());
    // optional: language and hours (`en 12`)
    if let Some(lang) = std::env::args().nth(2) {
        minitoo::i18n::set_language(&lang);
    }
    minitoo::i18n::set_formats(&std::env::args().nth(3).unwrap_or_else(|| "auto".into()), "auto");
    let dir = Path::new(&out);
    std::fs::create_dir_all(dir).unwrap();

    // clock: three faces without and with weather
    let mut clock = Clock::new();
    for (style, t) in [(0, at(0, 47, 32)), (1, at(0, 47, 36)), (2, at(0, 47, 41))] {
        clock.set_style(style);
        save(dir, &format!("nocity{style}_clock"), &clock.frame_at(t));
    }
    clock.set_weather(Some(Weather { temp: 9.5, code: 2, is_day: false, tmax: 14.1, tmin: 5.1 }));
    for (style, t) in [(0, at(0, 47, 48)), (1, at(0, 47, 55)), (2, at(0, 48, 2))] {
        clock.set_style(style);
        save(dir, &format!("city{style}_clock"), &clock.frame_at(t));
    }
    // every weather icon on the sky face
    for (i, (code, day)) in [(0, true), (0, false), (3, true), (45, true), (61, true), (71, true), (95, true), (81, false)]
        .into_iter()
        .enumerate()
    {
        clock.set_style(0);
        clock.set_weather(Some(Weather { temp: -3.4, code, is_day: day, tmax: 1.0, tmin: -7.5 }));
        save(dir, &format!("icon{i}_clock"), &clock.frame_at(at(14, 5, 0)));
    }

    // system monitor with the values of the reference capture
    let mut sm = SystemMonitor::new();
    let g = (1u64 << 30) as f64;
    sm.apply(&Reading {
        cpu: Some(0.0),
        cpu_temp: Some(37.0),
        mem_used: 5.5 * g,
        mem_total: 15.4 * g,
        gpu: Some(15.0),
        gpu_temp: Some(54.0),
        vram: Some((1.2 * g, 8.0 * g)),
    });
    save(dir, "sysmon_sysmon", &sm.frame(at(0, 46, 0)));
    for v in [12.0, 35.0, 80.0, 20.0, 55.0, 100.0, 5.0] {
        sm.apply(&Reading { cpu: Some(v), cpu_temp: Some(41.0), mem_used: 9.9 * g, mem_total: 31.0 * g, ..Default::default() });
    }
    save(dir, "sysmon_history", &sm.frame(at(0, 46, 0)));

    // pomodoro
    let mut p = Pomodoro::new();
    save(dir, "sysmon_pomodoro", &p.frame_at(25 * 60));
    save(dir, "pomodoro_half", &p.frame_at(12 * 60 + 30));
    p.skip();
    save(dir, "pomodoro_break", &p.frame_at(3 * 60 + 7));

    // github
    let mut gh = Github::new();
    gh.set_state(vec![], HashMap::new());
    save(dir, "sysmon_github", &gh.frame());
    let mut runs = HashMap::new();
    let ok = Run { status: "completed".into(), conclusion: "success".into(), ..Default::default() };
    runs.insert("cli/cli".to_string(), ok.clone());
    runs.insert("neovim/neovim".to_string(), ok);
    runs.insert("nonexistent-zz/nothing-zz".to_string(), Run { error: "не найден или приватный".into(), ..Default::default() });
    runs.insert("torvalds/linux".to_string(), Run { status: "none".into(), ..Default::default() });
    gh.set_state(
        vec!["cli/cli".into(), "neovim/neovim".into(), "nonexistent-zz/nothing-zz".into(), "torvalds/linux".into()],
        runs,
    );
    save(dir, "gh_github", &gh.frame());
    let mut runs = HashMap::new();
    runs.insert("a/failing".to_string(), Run { status: "completed".into(), conclusion: "failure".into(), ..Default::default() });
    runs.insert("a/running".to_string(), Run { status: "in_progress".into(), ..Default::default() });
    runs.insert("a/cancelled".to_string(), Run { status: "completed".into(), conclusion: "cancelled".into(), ..Default::default() });
    gh.set_state(vec!["a/failing".into(), "a/running".into(), "a/cancelled".into(), "a/loading".into()], runs);
    save(dir, "gh2_github", &gh.frame());

    // claude statistics
    let now = chrono::Local::now();
    let s = |id: &str, cwd: &str, state| Session {
        id: id.into(),
        cwd: cwd.into(),
        state,
        last_event: String::new(),
        message: String::new(),
        updated: now,
    };
    let sessions = vec![
        s("eeee5555", "/home/x/e", ClaudeState::Chilling),
        s("dddd4444", "", ClaudeState::Chilling),
        s("cccc3333", "/home/x/web", ClaudeState::Alerting),
        s("bbbb2222", "/home/x/very-long-project-name-here", ClaudeState::Working),
        s("aaaa1111", "/home/x/divoom-studio-rust", ClaudeState::Chilling),
    ];
    let totals = Totals { input: 1210, output: 4300, cache_read: 1_520_000, cache_write: 45000, replies: 2, prompts: 1 };
    save(dir, "stats_claudestats", &claudestats::frame(&sessions, &totals));
    save(dir, "stats0_claudestats", &claudestats::frame(&[], &totals));
    save(dir, "stats3_claudestats", &claudestats::frame(&sessions[2..4], &totals));

    // notification cards
    use minitoo::notify_card::render;
    save(dir, "card1", &render("Telegram", "Анна", "Созвон переносим на 20:30, ок?", "", "00:49"));
    save(
        dir,
        "card2",
        &render(
            "build",
            "Сборка готова с очень длинным заголовком",
            "<b>0 ошибок</b> &amp; 3 предупреждения, сборка заняла 2 минуты 14 секунд, артефакты загружены в хранилище, можно выкатывать на прод &lt;скоро&gt;",
            "",
            "00:49",
        ),
    );
    save(dir, "card3", &render("firefox", "", "Загрузка завершена", "", "00:49"));
    save(dir, "card4", &render("x", "Иконка-путь", "тест", "/usr/share/icons/hicolor/48x48/apps/firefox.png", "00:49"));
    save(dir, "card5", &render("Pomodoro", "Время перерыва", "Фокус завершён. Отдохните 5 мин.", "chronometer", "00:49"));
}
