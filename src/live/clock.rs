//! «Часы и погода» (`clock`, §8.2): three faces («Небо», «Неон», «Пиксели»), Open-Meteo weather and
//! city search, and "a minute ahead" device frames.

use super::{shortest_period, City, ClockView, LiveMode, ModeCommand, ModeCx, ModeMsg, ModeView};
use crate::canvas::{r, Align, Canvas, LineCap, LineJoin, R};
use crate::color::Color;
use crate::fonts::FontSpec;
use crate::frame::{Frame, HEIGHT, WIDTH};
use chrono::{Duration as ChronoDuration, NaiveDateTime, Timelike};
use std::time::Duration;
use tiny_skia::PathBuilder;

const TICK: u64 = 1;
const WEATHER: u64 = 2;
/// search debounce timers: `SEARCH_BASE + serial`
const SEARCH_BASE: u64 = 1 << 32;
const WEATHER_EVERY: Duration = Duration::from_secs(15 * 60);
const SEARCH_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";
const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weather {
    pub temp: f64,
    pub code: i64,
    pub is_day: bool,
    pub tmax: f64,
    pub tmin: f64,
}

enum Msg {
    Search { serial: u64, result: Result<Vec<City>, String> },
    Weather { serial: u64, result: Result<Weather, String> },
}

pub struct Clock {
    loaded: bool,
    style: i64,
    city: Option<City>,
    weather: Option<Weather>,
    results: Vec<City>,
    searching: bool,
    search_error: Option<String>,
    search_serial: u64,
    pending_query: String,
    weather_serial: u64,
    device_key: String,
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock {
    pub fn new() -> Self {
        Clock {
            loaded: false,
            style: 1,
            city: None,
            weather: None,
            results: Vec::new(),
            searching: false,
            search_error: None,
            search_serial: 0,
            pending_query: String::new(),
            weather_serial: 0,
            device_key: String::new(),
        }
    }

    fn ensure_loaded(&mut self, cx: &ModeCx) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let s = &*cx.settings;
        self.style = s.int("clock/style", 1).clamp(0, 2);
        self.city = s.opt_string("clock/city").map(|name| City {
            name,
            region: s.string("clock/region", ""),
            lat: s.float("clock/lat", 0.0),
            lon: s.float("clock/lon", 0.0),
        });
    }

    /// Face style (0 «Небо», 1 «Неон», 2 «Пиксели»), for offscreen rendering.
    pub fn set_style(&mut self, style: i64) {
        self.loaded = true;
        self.style = style.clamp(0, 2);
    }

    /// Current weather (for offscreen rendering and tests).
    pub fn set_weather(&mut self, w: Option<Weather>) {
        self.weather = w;
    }

    /// The face at a given local time.
    pub fn frame_at(&self, now: NaiveDateTime) -> Frame {
        match self.style {
            1 => self.render_neon(now),
            2 => self.render_pixel(now),
            _ => self.render_sky(now),
        }
    }

    fn device_key(&self, now: NaiveDateTime) -> String {
        let w = self.weather.unwrap_or(Weather { temp: 0.0, code: 0, is_day: true, tmax: 0.0, tmin: 0.0 });
        format!(
            "{} {} {} {} {} {} {} {}",
            now.format("%Y%m%d%H%M"),
            self.style,
            self.weather.is_some() as u8,
            qround(w.temp),
            qround(w.tmin),
            qround(w.tmax),
            w.code,
            w.is_day as u8
        )
    }

    fn schedule_tick(&self, cx: &mut ModeCx) {
        let ms = cx.now().timestamp_subsec_millis().min(999) as u64;
        cx.timer(Duration::from_millis(1000 - ms + 5), TICK);
    }

    fn fetch_weather(&mut self, cx: &mut ModeCx) {
        let Some(city) = self.city.clone() else {
            cx.set_status(tr!("clock.status.no_city"));
            return;
        };
        self.weather_serial += 1;
        let serial = self.weather_serial;
        let http = cx.http().clone();
        cx.spawn(async move {
            let result = fetch_forecast(&http, city.lat, city.lon).await;
            Box::new(Msg::Weather { serial, result }) as ModeMsg
        });
    }

    fn apply_city(&mut self, cx: &mut ModeCx, city: Option<City>, running: bool) {
        match &city {
            Some(c) => {
                cx.settings.set_string("clock/city", &c.name);
                cx.settings.set_string("clock/region", &c.region);
                cx.settings.set_float("clock/lat", c.lat);
                cx.settings.set_float("clock/lon", c.lon);
            }
            None => {
                cx.settings.set_string("clock/city", "");
                cx.settings.set_string("clock/region", "");
                cx.settings.set_float("clock/lat", 0.0);
                cx.settings.set_float("clock/lon", 0.0);
            }
        }
        self.city = city.filter(|c| !c.name.is_empty());
        self.weather = None;
        self.weather_serial += 1; // a forecast of the previous city is stale now
        if running {
            self.fetch_weather(cx);
        } else if self.city.is_none() {
            cx.set_status(tr!("clock.status.no_city"));
        }
        self.render(cx);
    }

    fn search(&mut self, cx: &mut ModeCx, query: &str) {
        let q = query.trim().to_string();
        self.search_serial += 1;
        self.search_error = None;
        if q.chars().count() < 2 {
            self.results.clear();
            self.searching = false;
            self.pending_query.clear();
            return;
        }
        self.searching = true;
        self.pending_query = q;
        cx.timer(Duration::from_millis(300), SEARCH_BASE + self.search_serial);
    }

    fn start_search_request(&mut self, cx: &mut ModeCx) {
        let serial = self.search_serial;
        let q = self.pending_query.clone();
        let http = cx.http().clone();
        cx.spawn(async move {
            let result = search_cities(&http, &q).await;
            Box::new(Msg::Search { serial, result }) as ModeMsg
        });
    }

    // ------------------------------------------------------------------ faces

    fn render_sky(&self, now: NaiveDateTime) -> Frame {
        let hour = now.hour();
        let night = match &self.weather {
            Some(w) => !w.is_day,
            None => !(7..20).contains(&hour),
        };
        let mut c = Canvas::device();
        let (top, bottom) = if night { (0x0a0e24, 0x1e224a) } else { (0x1c4080, 0x4a76ba) };
        c.vgradient(0.0, 0.0, WIDTH as f32, HEIGHT as f32, Color::hex(top), Color::hex(bottom));
        let y = if self.weather.is_some() { 8.0 } else { 26.0 };
        time_text(&mut c, r(0.0, y, 160.0, 46.0), &now, 42.0, Color::WHITE, |c, rect, s, f, col| c.text(rect, Align::CENTER, s, f, col));
        let date = crate::i18n::date_long(&now);
        let date_font = FontSpec::sans(11.0);
        c.text(r(0.0, y + 46.0, 160.0, 16.0), Align::CENTER, &Canvas::elide(&date, date_font, 156.0), date_font, Color::hex(0xcdd7f0));
        if let Some(w) = &self.weather {
            draw_weather_icon(&mut c, 14.0, 84.0, w.code, w.is_day);
            c.text(r(52.0, 82.0, 60.0, 22.0), Align::LEFT, &format!("{}°", qround(w.temp)), FontSpec::bold(20.0), Color::WHITE);
            let font = FontSpec::sans(10.0);
            c.text(r(52.0, 104.0, 70.0, 14.0), Align::LEFT, &Canvas::elide(weather_text(w.code), font, 70.0), font, Color::hex(0xcdd7f0));
            c.text(r(104.0, 84.0, 50.0, 14.0), Align::RIGHT, &format!("↑{}°", qround(w.tmax)), FontSpec::sans(10.0), Color::hex(0xffd2aa));
            c.text(r(104.0, 100.0, 50.0, 14.0), Align::RIGHT, &format!("↓{}°", qround(w.tmin)), FontSpec::sans(10.0), Color::hex(0xb4d2ff));
        }
        c.to_frame()
    }

    fn render_neon(&self, now: NaiveDateTime) -> Frame {
        let pink = Color::hex(0xff3cac);
        let cyan = Color::hex(0x38e8ff);
        let grid = Color::hex(0x280e34);
        let mut c = Canvas::device();
        c.fill(Color::hex(0x06040e));
        // faint perspective grid at the bottom, like a synthwave floor
        let mut y = 96;
        while y < HEIGHT as i32 {
            c.line(0.0, y as f32, WIDTH as f32, y as f32, 1.0, grid);
            y += 8;
        }
        let mut x = -80;
        while x <= 240 {
            c.line(80.0 + (x as f32 - 80.0) * 0.35, 96.0, x as f32, HEIGHT as f32, 1.0, grid);
            x += 20;
        }

        let date_font = FontSpec::bold(12.0);
        let date = format!("{} {}", crate::i18n::date_numeric(&now), crate::i18n::weekday_short(&now));
        glow_text(&mut c, r(0.0, 4.0, 160.0, 18.0), &Canvas::elide(&date, date_font, 154.0), date_font, cyan);
        // dotted cyan line (Qt DotLine, 1 px pen on y = 25 → two half-lit rows)
        c.blend_pixel(13, 24, cyan.with_alpha(64));
        c.blend_pixel(13, 25, cyan.with_alpha(64));
        let mut x = 16;
        while x <= 145 {
            c.blend_pixel(x, 24, cyan.with_alpha(128));
            c.blend_pixel(x, 25, cyan.with_alpha(128));
            x += 3;
        }

        if let Some(frame) = Canvas::round_rect_path(12.0, 30.0, 136.0, 50.0, 7.0) {
            glow_path(&mut c, &frame, pink, 2.0);
        }
        // the colon blinks: the digits stay put, only the colon goes
        let colon = now.second() % 2 == 1;
        time_text(&mut c, r(12.0, 30.0, 136.0, 50.0), &now, 38.0, pink, |c, rect, s, f, col| {
            let s = if colon || !s.contains(':') { s.to_string() } else { s.replacen(':', " ", 1) };
            glow_text(c, rect, &s, f, col)
        });

        if let Some(w) = &self.weather {
            let font = FontSpec::bold(12.0);
            let line = format!("{}°  {}", qround(w.temp), weather_text(w.code).to_uppercase());
            glow_text(&mut c, r(10.0, 88.0, 140.0, 18.0), &Canvas::elide(&line, font, 156.0), font, cyan);
            let mm = format!("↑{}°  ↓{}°", qround(w.tmax), qround(w.tmin));
            glow_text(&mut c, r(10.0, 106.0, 140.0, 16.0), &mm, FontSpec::sans(10.0), Color::hex(0xff96d2));
        } else {
            // heartbeat line
            let base = 104.0;
            let shift = (now.second() % 4 * 6) as i32;
            let mut pb = PathBuilder::new();
            pb.move_to(10.0, base);
            let mut x = 10.0f32;
            while x < 150.0 {
                let k = (x as i32 + shift) % 48;
                let y = match k {
                    20 => base - 14.0,
                    24 => base + 9.0,
                    28 => base - 4.0,
                    _ => base,
                };
                pb.line_to(x, y);
                x += 4.0;
            }
            if let Some(path) = pb.finish() {
                glow_path(&mut c, &path, cyan, 1.5);
            }
        }
        c.to_frame()
    }

    fn render_pixel(&self, now: NaiveDateTime) -> Frame {
        let lit = Color::hex(0xff7a42);
        let dim = Color::hex(0x1a110e);
        let mut c = Canvas::device();
        c.aa = false;
        c.fill(Color::hex(0x0a0706));
        let (cell, dot) = (8, 6);
        let top = if self.weather.is_some() { 10 } else { 22 };
        let digit = |c: &mut Canvas, d: u32, x0: i32| {
            for (row, line) in DIGITS[d as usize].iter().enumerate() {
                for (col, ch) in line.chars().enumerate() {
                    let color = if ch == '#' { lit } else { dim };
                    c.fill_rect((x0 + col as i32 * cell) as f32, (top + row as i32 * cell) as f32, dot as f32, dot as f32, color);
                }
            }
        };
        let x0 = (WIDTH as i32 - (4 * 3 * cell + 3 * cell + cell)) / 2 + 1;
        let twelve = crate::i18n::twelve_hours();
        let hour = if twelve { (now.hour() + 11) % 12 + 1 } else { now.hour() };
        if hour >= 10 || !twelve {
            digit(&mut c, hour / 10, x0);
        } else {
            // 12-hour time has no leading zero: the cells of the first digit stay dark
            for row in 0..5 {
                for col in 0..3 {
                    c.fill_rect((x0 + col * cell) as f32, (top + row * cell) as f32, dot as f32, dot as f32, dim);
                }
            }
        }
        digit(&mut c, hour % 10, x0 + 4 * cell);
        let colon = if now.second() % 2 == 0 { lit } else { dim };
        c.fill_rect((x0 + 8 * cell) as f32, (top + cell) as f32, dot as f32, dot as f32, colon);
        c.fill_rect((x0 + 8 * cell) as f32, (top + 3 * cell) as f32, dot as f32, dot as f32, colon);
        digit(&mut c, now.minute() / 10, x0 + 9 * cell);
        digit(&mut c, now.minute() % 10, x0 + 13 * cell);

        // seconds as a row of dashes
        let y = top + 5 * cell + 6;
        for i in 0..30 {
            let color = if (i as u32) < now.second() / 2 { lit.darker(1.2) } else { dim };
            c.fill_rect((5 + i * 5) as f32, y as f32, 3.0, 2.0, color);
        }

        let date = crate::i18n::date_short(&now).to_uppercase();
        let date = match crate::i18n::am_pm(&now) {
            Some(m) => tr!("clock.pixel.date_ampm", ampm = m, date = date),
            None => date,
        };
        pixel_text(&mut c, y + 8, &Canvas::elide(&date, FontSpec::bold(10.0).no_aa(), 156.0), 10.0, Color::hex(0xe6d2be));
        if let Some(w) = &self.weather {
            let line = format!("{}°  {}", qround(w.temp), weather_text(w.code));
            pixel_text(&mut c, y + 26, &Canvas::elide(&line, FontSpec::bold(10.0).no_aa(), 156.0), 10.0, lit);
            let mm = format!("↑{}° ↓{}°", qround(w.tmax), qround(w.tmin));
            pixel_text(&mut c, y + 42, &mm, 9.0, Color::hex(0xa08c7d));
        }
        c.to_frame()
    }
}

impl LiveMode for Clock {
    fn id(&self) -> &'static str {
        "clock"
    }
    fn title(&self) -> &'static str {
        tr!("clock.title")
    }
    fn subtitle(&self) -> &'static str {
        tr!("clock.subtitle")
    }
    fn icon(&self) -> &'static str {
        "clock"
    }
    fn owns_device_signal(&self) -> bool {
        true
    }

    fn start(&mut self, cx: &mut ModeCx) {
        self.ensure_loaded(cx);
        // timers and replies of a previous run were dropped
        self.searching = false;
        self.schedule_tick(cx);
        cx.timer(WEATHER_EVERY, WEATHER);
        self.fetch_weather(cx);
    }

    fn render(&mut self, cx: &mut ModeCx) {
        self.ensure_loaded(cx);
        let now = cx.now().naive_local();
        cx.publish(self.frame_at(now));
        let key = self.device_key(now);
        if key != self.device_key {
            self.device_key = key;
            cx.device_frames_changed();
        }
    }

    fn on_timer(&mut self, cx: &mut ModeCx, token: u64) {
        match token {
            TICK => {
                self.render(cx);
                self.schedule_tick(cx);
            }
            WEATHER => {
                self.fetch_weather(cx);
                cx.timer(WEATHER_EVERY, WEATHER);
            }
            t if t >= SEARCH_BASE => {
                if t - SEARCH_BASE == self.search_serial && self.searching {
                    self.start_search_request(cx);
                }
            }
            _ => {}
        }
    }

    fn on_message(&mut self, cx: &mut ModeCx, msg: ModeMsg) {
        let Ok(msg) = msg.downcast::<Msg>() else { return };
        match *msg {
            Msg::Search { serial, result } => {
                if serial != self.search_serial {
                    return; // a newer query is on its way
                }
                self.searching = false;
                match result {
                    Ok(list) => {
                        self.results = list;
                        self.search_error = None;
                    }
                    Err(e) => {
                        self.results.clear();
                        self.search_error = Some(e);
                    }
                }
            }
            Msg::Weather { serial, result } => {
                if serial != self.weather_serial {
                    return;
                }
                match result {
                    Ok(w) => {
                        self.weather = Some(w);
                        let name = self.city.as_ref().map(|c| c.name.clone()).unwrap_or_default();
                        let at = crate::i18n::time_hm(&cx.now());
                        cx.set_status(tr!("clock.status.weather", city = name, temp = qround(w.temp), sky = weather_text(w.code), time = at));
                        self.render(cx);
                    }
                    Err(e) => cx.set_status(tr!("clock.status.weather_error", error = e)),
                }
            }
        }
    }

    fn device_frames(&mut self, cx: &mut ModeCx) -> Option<(Vec<Frame>, u32)> {
        self.ensure_loaded(cx);
        let now = cx.now().naive_local();
        Some((self.minute_frames(now), 1000))
    }

    fn command(&mut self, cx: &mut ModeCx, cmd: ModeCommand) {
        self.ensure_loaded(cx);
        // timers and requests only run while the mode is acquired; the host drops them otherwise
        match cmd {
            ModeCommand::ClockStyle(s) => {
                let s = s.clamp(0, 2);
                if s != self.style {
                    self.style = s;
                    cx.settings.set_int("clock/style", s);
                    self.render(cx);
                }
            }
            ModeCommand::ClockSearch(q) => self.search(cx, &q),
            ModeCommand::ClockPickCity(city) => {
                self.results.clear();
                self.searching = false;
                self.search_serial += 1;
                self.apply_city(cx, Some(city), true);
            }
            ModeCommand::ClockClearCity => self.apply_city(cx, None, true),
            _ => {}
        }
    }

    fn view(&self) -> ModeView {
        ModeView::Clock(ClockView {
            style: self.style,
            city: self.city.clone(),
            results: self.results.clone(),
            searching: self.searching,
            search_error: self.search_error.clone(),
        })
    }
}

impl Clock {
    /// Frames from the current second to :59, reduced to their shortest period.
    pub fn minute_frames(&self, now: NaiveDateTime) -> Vec<Frame> {
        let second = now.with_nanosecond(0).unwrap_or(now);
        let s0 = second.second() as i64;
        let frames: Vec<Frame> = (s0..60).map(|s| self.frame_at(second + ChronoDuration::seconds(s - s0))).collect();
        shortest_period(frames)
    }
}

// ---------------------------------------------------------------------- network

async fn search_cities(http: &reqwest::Client, q: &str) -> Result<Vec<City>, String> {
    let url = reqwest::Url::parse_with_params(SEARCH_URL, &[("name", q), ("count", "8"), ("language", crate::i18n::current().code.as_str())])
        .map_err(|e| e.to_string())?;
    let resp = http.get(url).send().await.map_err(net_error)?;
    if !resp.status().is_success() {
        return Err(tr!("clock.error.http", status = resp.status().as_u16()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(parse_cities(&v))
}

/// Cities of a geocoding reply.
pub fn parse_cities(v: &serde_json::Value) -> Vec<City> {
    v.get("results").and_then(|r| r.as_array()).map(|a| a.iter().map(city_from_json).collect()).unwrap_or_default()
}

fn city_from_json(c: &serde_json::Value) -> City {
    let s = |k: &str| c.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let name = s("name");
    let fields: Vec<String> = ["country", "admin1", "admin2"].iter().map(|k| s(k)).collect();
    City {
        region: format_region(&name, &fields),
        name,
        lat: c.get("latitude").and_then(|v| v.as_f64()).unwrap_or(0.0),
        lon: c.get("longitude").and_then(|v| v.as_f64()).unwrap_or(0.0),
    }
}

/// Up to two of country / admin1 / admin2: non-empty, no repeats, not equal to the name.
pub fn format_region(name: &str, fields: &[String]) -> String {
    let mut out: Vec<&str> = Vec::new();
    for f in fields {
        if !f.is_empty() && !out.contains(&f.as_str()) && f != name {
            out.push(f);
        }
    }
    out.truncate(2);
    out.join(", ")
}

async fn fetch_forecast(http: &reqwest::Client, lat: f64, lon: f64) -> Result<Weather, String> {
    let (lat, lon) = (lat.to_string(), lon.to_string());
    let url = reqwest::Url::parse_with_params(
        FORECAST_URL,
        &[
            ("latitude", lat.as_str()),
            ("longitude", lon.as_str()),
            ("current", "temperature_2m,weather_code,is_day"),
            ("daily", "temperature_2m_max,temperature_2m_min"),
            ("forecast_days", "1"),
            ("timezone", "auto"),
        ],
    )
    .map_err(|e| e.to_string())?;
    let resp = http.get(url).send().await.map_err(net_error)?;
    if !resp.status().is_success() {
        return Err(tr!("clock.error.http", status = resp.status().as_u16()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    parse_forecast(&v).ok_or_else(|| tr!("clock.error.reply").to_string())
}

fn net_error(e: reqwest::Error) -> String {
    if e.is_timeout() {
        tr!("clock.error.timeout").to_string()
    } else if e.is_connect() {
        tr!("clock.error.connect").to_string()
    } else {
        e.to_string()
    }
}

/// Weather of a forecast reply.
pub fn parse_forecast(v: &serde_json::Value) -> Option<Weather> {
    let cur = v.get("current")?;
    let daily = v.get("daily");
    let first = |k: &str| daily.and_then(|d| d.get(k)).and_then(|a| a.get(0)).and_then(|x| x.as_f64()).unwrap_or(0.0);
    Some(Weather {
        temp: cur.get("temperature_2m")?.as_f64()?,
        code: cur.get("weather_code").and_then(|x| x.as_f64()).unwrap_or(0.0) as i64,
        is_day: cur.get("is_day").and_then(|x| x.as_f64()).unwrap_or(1.0) as i64 == 1,
        tmax: first("temperature_2m_max"),
        tmin: first("temperature_2m_min"),
    })
}

// ---------------------------------------------------------------------- text helpers

/// WMO weather code → a short description.
pub fn weather_text(code: i64) -> &'static str {
    match code {
        0 => tr!("clock.weather.clear"),
        1..=2 => tr!("clock.weather.partly_cloudy"),
        3 => tr!("clock.weather.overcast"),
        45 | 48 => tr!("clock.weather.fog"),
        51..=57 => tr!("clock.weather.drizzle"),
        61..=67 => tr!("clock.weather.rain"),
        71..=77 => tr!("clock.weather.snow"),
        80..=82 => tr!("clock.weather.showers"),
        85 | 86 => tr!("clock.weather.snowfall"),
        c if c >= 95 => tr!("clock.weather.thunderstorm"),
        _ => "",
    }
}

/// `qRound`: half away from zero.
fn qround(v: f64) -> i64 {
    v.round() as i64
}

/// Big clock digits centred in `rect`, with a small AM/PM after them when time is shown in 12
/// hours; the digits shrink if both do not fit. `draw` paints one text in a rect, centred.
fn time_text(c: &mut Canvas, rect: R, now: &NaiveDateTime, px: f32, color: Color, mut draw: impl FnMut(&mut Canvas, R, &str, FontSpec, Color)) {
    let digits = crate::i18n::clock_digits(now);
    let Some(marker) = crate::i18n::am_pm(now) else {
        draw(c, rect, &digits, FontSpec::bold(px), color);
        return;
    };
    let mark_font = FontSpec::bold((px * 0.3).round().max(9.0));
    let mark_w = Canvas::text_width(marker, mark_font).ceil();
    let gap = 3.0;
    let mut font = FontSpec::bold(px);
    while font.px > 12.0 && Canvas::text_width(&digits, font) + gap + mark_w > rect.w - 6.0 {
        font = FontSpec::bold(font.px - 1.0);
    }
    let dw = Canvas::text_width(&digits, font).ceil();
    let x = rect.x + ((rect.w - dw - gap - mark_w) / 2.0).round();
    draw(c, r(x, rect.y, dw, rect.h), &digits, font, color);
    // the marker sits on the baseline of the digits
    let base = rect.y + ((rect.h - font.height()) / 2.0).round() + font.ascent().round();
    let my = base - mark_font.ascent().round();
    draw(c, r(x + dw + gap, my, mark_w, mark_font.height().ceil()), marker, mark_font, color);
}

// ---------------------------------------------------------------------- drawing helpers

/// 3×5 digits for the LED-matrix face.
const DIGITS: [[&str; 5]; 10] = [
    ["###", "#.#", "#.#", "#.#", "###"],
    [".#.", "##.", ".#.", ".#.", "###"],
    ["###", "..#", "###", "#..", "###"],
    ["###", "..#", "###", "..#", "###"],
    ["#.#", "#.#", "###", "..#", "..#"],
    ["###", "#..", "###", "..#", "###"],
    ["###", "#..", "###", "#.#", "###"],
    ["###", "..#", ".#.", ".#.", ".#."],
    ["###", "#.#", "###", "#.#", "###"],
    ["###", "#.#", "###", "..#", "###"],
];

/// Text with a soft halo: 6 shifted copies at alpha 55, then the text itself.
fn glow_text(c: &mut Canvas, rect: R, s: &str, font: FontSpec, color: Color) {
    let halo = color.with_alpha(55);
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-2.0, 0.0), (2.0, 0.0)] {
        c.text(r(rect.x + dx, rect.y + dy, rect.w, rect.h), Align::CENTER, s, font, halo);
    }
    c.text(rect, Align::CENTER, s, font, color);
}

/// A stroked path with a wider translucent halo under it.
fn glow_path(c: &mut Canvas, path: &tiny_skia::Path, color: Color, width: f32) {
    c.stroke_path_with(path, width + 4.0, color.with_alpha(60), LineCap::Round, LineJoin::Round);
    c.stroke_path_with(path, width, color, LineCap::Round, LineJoin::Round);
}

/// Bold, non-antialiased text centred horizontally with its top at `y` (pixelText of the Qt
/// version: an image as wide as the advance, drawn at `(160 - w) / 2`).
fn pixel_text(c: &mut Canvas, y: i32, s: &str, px: f32, color: Color) {
    let font = FontSpec::bold(px).no_aa();
    let w = Canvas::text_width(s, font).round().max(1.0) as i32;
    let x = (WIDTH as i32 - w) / 2;
    c.text_at(x as f32, y as f32 + font.ascent().round(), s, font, color);
}

fn draw_cloud(c: &mut Canvas, ox: f32, oy: f32, s: f32, color: Color) {
    let mut pb = PathBuilder::new();
    let oval = |pb: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32| {
        if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) {
            pb.push_oval(rect);
        }
    };
    oval(&mut pb, ox, oy + 6.0 * s, 14.0 * s, 10.0 * s);
    oval(&mut pb, ox + 6.0 * s, oy, 14.0 * s, 14.0 * s);
    oval(&mut pb, ox + 14.0 * s, oy + 5.0 * s, 12.0 * s, 11.0 * s);
    if let Some(rect) = tiny_skia::Rect::from_xywh(ox + 6.0 * s, oy + 9.0 * s, 14.0 * s, 7.0 * s) {
        pb.push_rect(rect);
    }
    if let Some(path) = pb.finish() {
        // one path filled with the winding rule = the union of the shapes
        c.fill_path(&path, color);
    }
}

/// Vector weather icon (about 28×28) at (ox, oy).
pub fn draw_weather_icon(c: &mut Canvas, ox: f32, oy: f32, code: i64, day: bool) {
    let sun = Color::rgb(255, 196, 64);
    let moon = Color::rgb(240, 232, 190);
    let cloud = Color::rgb(232, 236, 246);
    let dark = Color::rgb(150, 158, 178);
    if code <= 2 {
        if day {
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::PI / 4.0;
                c.line_cap(
                    ox + 12.0 + 10.0 * a.cos(),
                    oy + 12.0 + 10.0 * a.sin(),
                    ox + 12.0 + 13.0 * a.cos(),
                    oy + 12.0 + 13.0 * a.sin(),
                    2.0,
                    sun,
                    LineCap::Square,
                );
            }
            c.fill_circle(ox + 12.0, oy + 12.0, 7.5, sun);
        } else {
            // crescent: a disc minus a shifted disc, drawn on a layer and composed
            let mut layer = Canvas::new(c.width(), c.height());
            layer.fill_circle(ox + 12.0, oy + 12.0, 9.0, moon);
            if let Some(cut) = PathBuilder::from_circle(ox + 17.0, oy + 8.0, 8.0) {
                let paint = tiny_skia::Paint { anti_alias: true, blend_mode: tiny_skia::BlendMode::Clear, ..Default::default() };
                layer.pixmap_mut().fill_path(&cut, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
            }
            c.draw_canvas(&layer, 0.0, 0.0, 1.0);
        }
    }
    if code >= 1 {
        let small = code <= 2;
        let (cx, cy) = (ox + if small { 6.0 } else { 0.0 }, oy + if small { 10.0 } else { 4.0 });
        let color = if code >= 61 || code == 3 { dark.lighter(1.3) } else { cloud };
        draw_cloud(c, cx, cy, if small { 0.85 } else { 1.1 }, color);
        let rain = Color::rgb(110, 170, 255);
        if (51..=67).contains(&code) || (80..=82).contains(&code) {
            for i in 0..3 {
                let i = i as f32;
                c.line_cap(cx + 7.0 + i * 6.0, cy + 21.0, cx + 4.0 + i * 6.0, cy + 27.0, 2.0, rain, LineCap::Round);
            }
        }
        if (71..=77).contains(&code) || code == 85 || code == 86 {
            for i in 0..3 {
                c.fill_circle(cx + 6.0 + i as f32 * 7.0, cy + 24.0 + (i % 2) as f32 * 3.0, 1.8, Color::WHITE);
            }
        }
        if code >= 95 {
            let (bx, by) = (cx + 12.0, cy + 18.0);
            let pts = [(0.0, 0.0), (-5.0, 8.0), (-1.0, 8.0), (-4.0, 15.0), (4.0, 5.0), (0.0, 5.0)];
            let poly: Vec<(f32, f32)> = pts.iter().map(|(x, y)| (bx + x, by + y)).collect();
            c.fill_polygon(&poly, Color::rgb(255, 210, 60));
        }
        if code == 45 || code == 48 {
            for i in 0..2 {
                let y = cy + 21.0 + i as f32 * 5.0;
                c.line_cap(cx + 2.0, y, cx + 26.0, y, 2.0, cloud, LineCap::Round);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(h: u32, m: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 9).unwrap().and_hms_opt(h, m, s).unwrap()
    }

    #[test]
    fn weather_codes() {
        assert_eq!(weather_text(0), "ясно");
        assert_eq!(weather_text(2), "малооблачно");
        assert_eq!(weather_text(3), "пасмурно");
        assert_eq!(weather_text(48), "туман");
        assert_eq!(weather_text(55), "морось");
        assert_eq!(weather_text(63), "дождь");
        assert_eq!(weather_text(75), "снег");
        assert_eq!(weather_text(81), "ливень");
        assert_eq!(weather_text(86), "снегопад");
        assert_eq!(weather_text(99), "гроза");
        assert_eq!(weather_text(10), "");
    }

    #[test]
    fn region_rule() {
        let f = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(format_region("Москва", &f(&["Россия", "Москва", "Москва"])), "Россия");
        assert_eq!(format_region("Химки", &f(&["Россия", "Московская область", "городской округ Химки"])), "Россия, Московская область");
        assert_eq!(format_region("X", &f(&["", "A", "A"])), "A");
        let v: serde_json::Value = serde_json::from_str(
            r#"{"results":[{"name":"Астана","latitude":51.18,"longitude":71.45,"country":"Казахстан","admin1":"Астана"}]}"#,
        )
        .unwrap();
        let c = parse_cities(&v);
        assert_eq!(c[0].name, "Астана");
        assert_eq!(c[0].region, "Казахстан");
        assert_eq!(c[0].lat, 51.18);
        assert!(parse_cities(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn forecast_parsing() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"current":{"temperature_2m":9.5,"weather_code":2,"is_day":0},"daily":{"temperature_2m_max":[14.1],"temperature_2m_min":[5.1]}}"#,
        )
        .unwrap();
        let w = parse_forecast(&v).unwrap();
        assert_eq!((qround(w.temp), w.code, w.is_day, qround(w.tmax), qround(w.tmin)), (10, 2, false, 14, 5));
        assert_eq!(qround(-2.5), -3);
    }

    #[test]
    fn russian_dates() {
        assert_eq!(crate::i18n::date_long(&at(0, 47, 0)), "пятница, 9 октября");
        assert_eq!(crate::i18n::date_short(&at(0, 47, 0)).to_uppercase(), "ПТ, 9 ОКТЯБРЯ");
    }

    #[test]
    fn device_frame_periods() {
        let mut c = Clock::new();
        c.set_style(1);
        // without weather the heartbeat line repeats every 4 s
        assert_eq!(c.minute_frames(at(12, 30, 10)).len(), 4);
        assert_eq!(c.minute_frames(at(12, 30, 59)).len(), 1);
        // «Пиксели» changes every 2 s with the seconds bar: no period
        c.set_style(2);
        assert_eq!(c.minute_frames(at(12, 30, 10)).len(), 50);
        // «Небо» is static within a minute
        c.set_style(0);
        assert_eq!(c.minute_frames(at(12, 30, 10)).len(), 1);
        // «Неон» with weather only blinks the colon: two frames
        c.set_style(1);
        c.set_weather(Some(Weather { temp: 9.5, code: 2, is_day: false, tmax: 14.1, tmin: 5.1 }));
        assert_eq!(c.minute_frames(at(12, 30, 10)).len(), 2);
    }

    #[test]
    fn device_key_changes_per_minute() {
        let c = Clock::new();
        assert_eq!(c.device_key(at(1, 2, 3)), c.device_key(at(1, 2, 59)));
        assert_ne!(c.device_key(at(1, 2, 3)), c.device_key(at(1, 3, 0)));
    }
}
