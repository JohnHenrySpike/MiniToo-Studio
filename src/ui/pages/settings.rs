//! «Настройки» (§16.12): theme, language and formats, connection, transfer, application.

use super::scroll_page;
use crate::api::{Command, Theme};
use crate::ui::Cx;
use crate::ui::theme::pal;
use crate::ui::widgets::{self as w, Field, Key, Spin};
use egui::text::LayoutJob;
use egui::{Rect, Sense, Ui, pos2, vec2};

#[derive(Default)]
pub struct State {
    mac: Option<String>,
    /// the date pattern being typed (shown while «Свой формат» is chosen)
    date_pattern: Option<String>,
    /// «Свой формат» was picked while a preset was in use
    custom_date: bool,
}

fn label_cell(ui: &mut Ui, text: &str, width: f32, height: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    if !text.is_empty() {
        let g = w::galley(ui, text, w::font(13.0), pal().text);
        ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, pal().text);
    }
}

/// A grid row: label column, then the content.
fn grid_row(ui: &mut Ui, label: &str, lw: f32, add: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        label_cell(ui, label, lw, 33.0);
        ui.vertical(|ui| add(ui));
    });
}

fn spin_hint(ui: &mut Ui, cx: &Cx, spin: Spin, hint: &str, cmd: impl Fn(i64) -> Command) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        if let Some(v) = spin.show(ui) {
            cx.send(cmd(v));
        }
        let rest = ui.available_width();
        let g = w::galley_wrapped(ui, hint, w::font(13.0), pal().text_dim, rest);
        let h = g.size().y;
        let (r, _) = ui.allocate_exact_size(vec2(rest, h.max(33.0)), Sense::hover());
        ui.painter().galley(pos2(r.left(), r.center().y - h / 2.0), g, pal().text_dim);
    });
}

fn cli_hint(ui: &mut Ui) {
    let p = pal();
    let parts: [(&str, bool); 11] = [
        (tr!("settings.cli.prefix"), false),
        (tr!("settings.cli.send"), true),
        (", ", false),
        ("--state working|alerting|chilling", true),
        (", ", false),
        ("--status", true),
        (", ", false),
        ("--hidden", true),
        (", ", false),
        ("--debug", true),
        (tr!("settings.cli.debug"), false),
    ];
    let width = ui.available_width();
    let mut job = LayoutJob::default();
    for (t, mono) in parts {
        let font = if mono { w::mono(12.0) } else { w::font(13.0) };
        let fmt = egui::TextFormat {
            extra_letter_spacing: if mono { 0.0 } else { w::tight(13.0) },
            font_id: font,
            color: p.text_dim,
            valign: egui::Align::BOTTOM,
            ..Default::default()
        };
        job.append(t, 0.0, fmt);
    }
    job.wrap.max_width = width;
    let g = ui.fonts_mut(|f| f.layout_job(job));
    let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
    ui.painter().galley(r.min, g, p.text_dim);
}

/// Opens the folder of user translations, with a fresh copy of the English catalog to start
/// from (`en.lang.template`, not loaded itself).
fn open_locales_dir() {
    let dir = crate::i18n::locales_dir();
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join("en.lang.template"), crate::i18n::english_template())) {
        log::error!("{}: {e}", dir.display());
        return;
    }
    let opener = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(opener).arg(&dir).spawn() {
        log::error!("{opener}: {e}");
    }
}

fn language_group(ui: &mut Ui, cx: &mut Cx, st: &mut State, lw: f32) {
    let p = pal();
    let s = &cx.snap.settings;
    let now = chrono::Local::now();
    w::group(ui, tr!("settings.group.language"), |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;

        // language: «as in the system» first, then every catalog
        let langs = crate::i18n::available();
        let system = crate::i18n::system_language();
        let system_name = langs.iter().find(|(c, _)| *c == system).map(|(_, n)| n.as_str()).unwrap_or("English");
        let mut items = vec![tr!("settings.language.system", language = system_name)];
        items.extend(langs.iter().map(|(_, n)| n.clone()));
        let cur = if s.language == "auto" { 0 } else { langs.iter().position(|(c, _)| *c == s.language).map(|i| i + 1).unwrap_or(0) };
        grid_row(ui, tr!("settings.language"), lw, |ui| {
            w::row(ui, 33.0, 12.0, |ui| {
                let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
                if let Some(i) = w::combo(ui, "language", &refs, cur, 220.0) {
                    let code = if i == 0 { "auto".to_string() } else { langs[i - 1].0.clone() };
                    cx.send(Command::SetLanguage(code));
                }
                if Key::new(tr!("settings.language.folder")).icon("open").tip(tr!("settings.language.folder_tip")).show(ui).clicked() {
                    open_locales_dir();
                }
            });
        });

        // time: as the language says, 24 or 12 hours
        let cur = match s.time_format.as_str() {
            "24" => 1,
            "12" => 2,
            _ => 0,
        };
        grid_row(ui, tr!("settings.time"), lw, |ui| {
            w::row(ui, 33.0, 12.0, |ui| {
                let items = [(tr!("settings.format.auto"), None), (tr!("settings.time.24"), None), (tr!("settings.time.12"), None)];
                if let Some(i) = w::tabs(ui, &items, cur) {
                    cx.send(Command::SetTimeFormat(["auto", "24", "12"][i].to_string()));
                }
                w::para(ui, &crate::i18n::time_hm(&now), w::font(13.0), p.text_dim);
            });
        });

        // date: presets shown as today's date, or a pattern of one's own
        let presets = crate::i18n::DATE_PRESETS;
        let current = if s.date_format == "auto" { "" } else { s.date_format.as_str() };
        let preset = presets.iter().position(|pr| *pr == current);
        let custom = st.custom_date || preset.is_none();
        let mut items: Vec<String> = presets
            .iter()
            .map(|pr| {
                if pr.is_empty() {
                    tr!("settings.date.auto", example = crate::i18n::format_date(&now, tr!("format.date")))
                } else {
                    crate::i18n::format_date(&now, pr)
                }
            })
            .collect();
        items.push(tr!("settings.date.custom").to_string());
        grid_row(ui, tr!("settings.date"), lw, |ui| {
            w::row(ui, 33.0, 12.0, |ui| {
                let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
                let cur = if custom { presets.len() } else { preset.unwrap_or(0) };
                if let Some(i) = w::combo(ui, "date-format", &refs, cur, 280.0) {
                    if i == presets.len() {
                        st.custom_date = true;
                        st.date_pattern = Some(crate::i18n::date_pattern());
                    } else {
                        st.custom_date = false;
                        st.date_pattern = None;
                        cx.send(Command::SetDateFormat(if presets[i].is_empty() { "auto".into() } else { presets[i].to_string() }));
                    }
                }
                if custom {
                    let buf = st.date_pattern.get_or_insert_with(|| current.to_string());
                    let r = w::text_field(ui, buf, Field { hint: "%d.%m.%Y", width: 140.0, mono: true, ..Default::default() });
                    if r.lost_focus() && !buf.trim().is_empty() && buf.trim() != current {
                        cx.send(Command::SetDateFormat(buf.trim().to_string()));
                    }
                    let example = crate::i18n::format_date(&now, buf.trim());
                    w::para(ui, &example, w::font(13.0), p.text_dim);
                }
            });
            if custom {
                w::hint_small(ui, tr!("settings.date.pattern_hint"), 11.0);
            }
        });
        w::hint_small(ui, &tr!("settings.language.hint", folder = crate::i18n::locales_dir().display()), 11.0);
    });
}

pub fn show(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    let p = pal();
    scroll_page(ui, "settings-page", Some(860.0), |ui| {
        w::page_header(ui, tr!("settings.title"), tr!("settings.subtitle"), |_| {});

        w::group(ui, tr!("settings.group.look"), |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let cur = if cx.snap.theme == Theme::Dark { 1 } else { 0 };
                if let Some(i) = w::tabs(ui, &[(tr!("settings.theme.beige"), Some("sun")), (tr!("settings.theme.night"), Some("moon"))], cur) {
                    cx.theme_request = Some(if i == 1 { Theme::Dark } else { Theme::Beige });
                }
                let rest = ui.available_width();
                w::para_w(ui, tr!("settings.theme.hint"), w::font(13.0), p.text_dim, rest, egui::Align::Min);
            });
        });

        let labels = [tr!("settings.language"), tr!("settings.time"), tr!("settings.date"), tr!("settings.mac"), tr!("settings.channel"), tr!("settings.keepalive")];
        let lw = labels.iter().map(|l| w::text_width(ui, l, w::font(13.0))).fold(0.0, f32::max) + 4.0;
        language_group(ui, cx, st, lw);

        let s = &cx.snap.settings;
        let d = &cx.snap.device;
        w::group(ui, tr!("settings.group.device"), |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            grid_row(ui, tr!("settings.mac"), lw, |ui| {
                w::row(ui, 33.0, 12.0, |ui| {
                    let key = Key::new(if d.discovering { tr!("settings.mac.searching") } else { tr!("settings.mac.find") }).icon("search").enabled(!d.discovering);
                    let kw = key.size(ui).x;
                    let buf = st.mac.get_or_insert_with(|| s.mac.clone());
                    let r = w::text_field(ui, buf, Field { hint: "B1:21:81:05:E2:65", width: ui.available_width() - kw - 12.0, mono: true, ..Default::default() });
                    if r.lost_focus() {
                        let v = buf.trim().to_uppercase();
                        if v != s.mac {
                            cx.send(Command::SetMac(v));
                        }
                    }
                    if !r.has_focus() && !r.lost_focus() && *buf != s.mac {
                        *buf = s.mac.clone();
                    }
                    if key.show(ui).clicked() {
                        cx.send(Command::Discover);
                    }
                });
                if !d.discovered.is_empty() {
                    let width = ui.available_width();
                    let h = d.discovered.len() as f32 * 34.0 + 8.0;
                    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
                    w::paint_panel(ui.painter(), rect);
                    let inner = Rect::from_min_max(rect.min + vec2(4.0, 4.0), rect.max - vec2(4.0, 4.0));
                    let mut picked = None;
                    super::area(ui, inner, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for (name, addr) in &d.discovered {
                            let n = if name.is_empty() { tr!("settings.mac.unnamed") } else { name };
                            if w::item_row(ui, &format!("{n}   {addr}"), *addr == s.mac, inner.width()).clicked() {
                                picked = Some(addr.clone());
                            }
                        }
                    });
                    if let Some(a) = picked {
                        st.mac = Some(a.clone());
                        cx.send(Command::SetMac(a));
                    }
                }
            });
            grid_row(ui, tr!("settings.channel"), lw, |ui| {
                if let Some(v) = Spin::new("rfcomm", s.channel as i64, 1, 30).show(ui) {
                    cx.send(Command::SetChannel(v as u8));
                }
            });
            grid_row(ui, "", lw, |ui| {
                if w::checkbox(ui, s.auto_connect, tr!("settings.auto_connect"), true).clicked() {
                    cx.send(Command::SetAutoConnect(!s.auto_connect));
                }
            });
            grid_row(ui, tr!("settings.keepalive"), lw, |ui| {
                spin_hint(ui, cx, Spin::new("keepalive", s.keepalive as i64, 0, 600).step(10), tr!("settings.keepalive.hint"), |v| Command::SetKeepalive(v as u32));
            });
        });

        let labels = [tr!("settings.chunk_delay"), tr!("settings.zstd")];
        let lw = labels.iter().map(|l| w::text_width(ui, l, w::font(13.0))).fold(0.0, f32::max) + 4.0;
        w::group(ui, tr!("settings.group.transfer"), |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            grid_row(ui, tr!("settings.chunk_delay"), lw, |ui| {
                spin_hint(ui, cx, Spin::new("chunk-delay", s.chunk_delay as i64, 0, 60), tr!("settings.chunk_delay.hint"), |v| Command::SetChunkDelay(v as u32));
            });
            grid_row(ui, tr!("settings.zstd"), lw, |ui| {
                spin_hint(ui, cx, Spin::new("zstd", s.zstd_level as i64, 1, 22), tr!("settings.zstd.hint"), |v| Command::SetZstdLevel(v as i32));
            });
        });

        w::group(ui, tr!("settings.group.app"), |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            if w::checkbox(ui, s.close_to_tray, tr!("settings.close_to_tray"), true).clicked() {
                cx.send(Command::SetCloseToTray(!s.close_to_tray));
            }
            if w::checkbox(ui, s.start_hidden, tr!("settings.start_hidden"), true).clicked() {
                cx.send(Command::SetStartHidden(!s.start_hidden));
            }
            cli_hint(ui);
        });
    });
}
