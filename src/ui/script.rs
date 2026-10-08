//! Scripted input for checking the window without hands (used with `--screenshot DIR` and
//! `MINITOO_UI_SCRIPT`): `move X Y; down; up; click X Y; wheel DY; key Enter; text abc;
//! wait N; shot NAME; page N; dark; beige; close`. One step per frame.

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, pos2};
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub enum Step {
    Move(Pos2),
    Down,
    Up,
    Wheel(f32),
    Key(egui::Key),
    Text(String),
    Wait(u32),
    Shot(String),
    Page(usize),
    Dark(bool),
    Close,
}

pub struct Script {
    steps: VecDeque<Step>,
    pos: Pos2,
    wait: u32,
    /// a screenshot was requested and is not saved yet
    pub shooting: Option<String>,
}

/// What the app has to do for the current step.
pub enum Action {
    None,
    Shot(String),
    Page(usize),
    Dark(bool),
    Close,
}

impl Script {
    pub fn parse(src: &str) -> Script {
        let mut steps = VecDeque::new();
        for part in src.split(';') {
            let w: Vec<&str> = part.split_whitespace().collect();
            let num = |i: usize| w.get(i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            let step = match w.first().copied() {
                Some("move") => Step::Move(pos2(num(1), num(2))),
                Some("down") => Step::Down,
                Some("up") => Step::Up,
                Some("click") => {
                    steps.push_back(Step::Move(pos2(num(1), num(2))));
                    steps.push_back(Step::Wait(2));
                    steps.push_back(Step::Down);
                    steps.push_back(Step::Up);
                    Step::Wait(4)
                }
                Some("wheel") => Step::Wheel(num(1)),
                Some("key") => match w.get(1).and_then(|k| egui::Key::from_name(k)) {
                    Some(k) => Step::Key(k),
                    None => continue,
                },
                Some("text") => Step::Text(w[1..].join(" ")),
                Some("wait") => Step::Wait(num(1) as u32),
                Some("shot") => Step::Shot(w.get(1).unwrap_or(&"shot").to_string()),
                Some("page") => Step::Page(num(1) as usize),
                Some("dark") => Step::Dark(true),
                Some("beige") => Step::Dark(false),
                Some("close") => Step::Close,
                _ => continue,
            };
            steps.push_back(step);
        }
        Script { steps, pos: pos2(0.0, 0.0), wait: 0, shooting: None }
    }

    pub fn done(&self) -> bool {
        self.steps.is_empty() && self.shooting.is_none() && self.wait == 0
    }

    /// Injects this frame's events.
    pub fn hook(&mut self, raw: &mut RawInput) -> Action {
        if self.shooting.is_some() {
            return Action::None;
        }
        if self.wait > 0 {
            self.wait -= 1;
            return Action::None;
        }
        let Some(step) = self.steps.pop_front() else { return Action::None };
        let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        match step {
            Step::Move(p) => {
                self.pos = p;
                raw.events.push(Event::PointerMoved(p));
            }
            Step::Down => raw.events.push(button(self.pos, true)),
            Step::Up => raw.events.push(button(self.pos, false)),
            Step::Wheel(dy) => raw.events.push(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, dy),
                phase: egui::TouchPhase::Move,
                modifiers: Modifiers::NONE,
            }),
            Step::Key(k) => {
                raw.events.push(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
                raw.events.push(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
            }
            Step::Text(t) => raw.events.push(Event::Text(t)),
            Step::Wait(n) => self.wait = n,
            Step::Shot(name) => {
                self.shooting = Some(name.clone());
                return Action::Shot(name);
            }
            Step::Page(n) => return Action::Page(n),
            Step::Dark(d) => return Action::Dark(d),
            Step::Close => return Action::Close,
        }
        Action::None
    }
}
