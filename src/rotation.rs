//! Rotation of the ticked live modes (§9). A pure state machine: the controller calls `tick`
//! once a second (a full second after every switch) and acts on the returned switch.

pub const MIN_INTERVAL: u32 = 10;
pub const MAX_INTERVAL: u32 = 600;
pub const DEFAULT_INTERVAL: u32 = 30;

#[derive(Debug, Clone)]
pub struct Rotation {
    order: Vec<String>,
    members: Vec<String>,
    interval: u32,
    running: bool,
    paused: bool,
    current: String,
    remaining: u32,
}

impl Rotation {
    pub fn new(order: Vec<String>) -> Self {
        Rotation {
            order,
            members: Vec::new(),
            interval: DEFAULT_INTERVAL,
            running: false,
            paused: false,
            current: String::new(),
            remaining: 0,
        }
    }

    pub fn members(&self) -> &[String] {
        &self.members
    }
    pub fn count(&self) -> usize {
        self.members.len()
    }
    pub fn contains(&self, id: &str) -> bool {
        self.members.iter().any(|m| m == id)
    }
    pub fn interval(&self) -> u32 {
        self.interval
    }
    pub fn running(&self) -> bool {
        self.running
    }
    pub fn paused(&self) -> bool {
        self.paused
    }
    pub fn remaining(&self) -> u32 {
        self.remaining
    }
    pub fn current(&self) -> &str {
        &self.current
    }
    /// The second-timer should run.
    pub fn ticking(&self) -> bool {
        self.running && !self.paused && self.count() >= 2
    }

    fn pos(&self, id: &str) -> Option<usize> {
        self.order.iter().position(|o| o == id)
    }

    /// Sets the members (deduplicated, unknown ids dropped, in list order). Returns a mode to
    /// switch to, if the current one was unticked; `Stop` if none is left.
    pub fn set_members(&mut self, ids: &[String]) -> RotationEffect {
        let mut list: Vec<String> = Vec::new();
        for id in ids {
            if !list.contains(id) && (self.order.is_empty() || self.order.contains(id)) {
                list.push(id.clone());
            }
        }
        list.sort_by_key(|id| self.pos(id).unwrap_or(usize::MAX));
        if list == self.members {
            return RotationEffect::None;
        }
        let old = self.count();
        self.members = list;
        if !self.running {
            return RotationEffect::None;
        }
        if self.members.is_empty() {
            self.stop();
            return RotationEffect::Stopped;
        }
        if !self.contains(&self.current.clone()) {
            // the current mode was unticked: move on to the one after it
            let next = self.next_id();
            return self.show(next);
        }
        if old < 2 && self.count() >= 2 {
            self.remaining = self.interval; // a single mode just became a rotation
            return RotationEffect::Restart;
        }
        if self.count() < 2 {
            self.remaining = 0;
        }
        RotationEffect::None
    }

    pub fn set_member(&mut self, id: &str, on: bool) -> RotationEffect {
        if on == self.contains(id) {
            return RotationEffect::None;
        }
        let mut list = self.members.clone();
        if on {
            list.push(id.to_string());
        } else {
            list.retain(|m| m != id);
        }
        self.set_members(&list)
    }

    /// Returns `Restart` when the countdown restarted.
    pub fn set_interval(&mut self, sec: u32) -> RotationEffect {
        let sec = sec.clamp(MIN_INTERVAL, MAX_INTERVAL);
        if sec == self.interval {
            return RotationEffect::None;
        }
        self.interval = sec;
        if self.running && self.count() >= 2 {
            self.remaining = self.interval;
            return RotationEffect::Restart;
        }
        RotationEffect::None
    }

    /// The member after the current one in list order (wrapping).
    pub fn next_id(&self) -> String {
        if self.members.is_empty() {
            return String::new();
        }
        let cur = self.pos(&self.current);
        self.members
            .iter()
            .find(|id| match (self.pos(id), cur) {
                (Some(p), Some(c)) => p > c,
                (Some(_), None) => true,
                _ => false,
            })
            .cloned()
            .unwrap_or_else(|| self.members[0].clone())
    }

    /// Starts with `preferred` if it is ticked, else the first ticked mode.
    pub fn start(&mut self, preferred: &str) -> RotationEffect {
        if self.members.is_empty() {
            return RotationEffect::None;
        }
        self.running = true;
        self.paused = false;
        let id = if self.contains(preferred) { preferred.to_string() } else { self.members[0].clone() };
        self.show(id)
    }

    pub fn stop(&mut self) -> bool {
        if !self.running {
            return false;
        }
        self.running = false;
        self.paused = false;
        self.remaining = 0;
        true
    }

    pub fn next(&mut self) -> RotationEffect {
        if !self.running || self.members.is_empty() {
            return RotationEffect::None;
        }
        let id = self.next_id();
        self.show(id)
    }

    pub fn set_paused(&mut self, on: bool) -> bool {
        let on = on && self.running;
        if on == self.paused {
            return false;
        }
        self.paused = on;
        true
    }

    /// One second passed.
    pub fn tick(&mut self) -> RotationEffect {
        if !self.ticking() {
            return RotationEffect::None;
        }
        self.remaining = self.remaining.saturating_sub(1);
        if self.remaining == 0 {
            return self.next();
        }
        RotationEffect::None
    }

    fn show(&mut self, id: String) -> RotationEffect {
        self.current = id.clone();
        self.remaining = if self.count() >= 2 { self.interval } else { 0 };
        RotationEffect::Switch(id)
    }

    /// 0..1 towards the next switch.
    pub fn progress(&self) -> f32 {
        if !self.running || self.count() < 2 || self.interval == 0 {
            return 0.0;
        }
        1.0 - self.remaining as f32 / self.interval as f32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RotationEffect {
    None,
    /// show this mode now (and restart the 1-s timer)
    Switch(String),
    /// the countdown restarted: restart the 1-s timer
    Restart,
    Stopped,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rot() -> Rotation {
        let mut r = Rotation::new(["clock", "sysmon", "nowplaying", "pomodoro"].iter().map(|s| s.to_string()).collect());
        r.set_interval(10);
        r
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rotates_in_list_order() {
        let mut r = rot();
        r.set_members(&ids(&["pomodoro", "clock", "bogus", "clock"]));
        assert_eq!(r.members(), &ids(&["clock", "pomodoro"])[..]);
        assert_eq!(r.start("pomodoro"), RotationEffect::Switch("pomodoro".into()));
        for _ in 0..9 {
            assert_eq!(r.tick(), RotationEffect::None);
        }
        assert_eq!(r.tick(), RotationEffect::Switch("clock".into()));
        assert_eq!(r.remaining(), 10);
    }

    #[test]
    fn single_member_just_shows() {
        let mut r = rot();
        r.set_members(&ids(&["sysmon"]));
        assert_eq!(r.start("clock"), RotationEffect::Switch("sysmon".into()));
        assert!(!r.ticking());
        assert_eq!(r.set_member("clock", true), RotationEffect::Restart);
        assert!(r.ticking());
        assert_eq!(r.remaining(), 10);
    }

    #[test]
    fn untick_current_moves_on_and_pause_freezes() {
        let mut r = rot();
        r.set_members(&ids(&["clock", "sysmon", "pomodoro"]));
        r.start("sysmon");
        assert_eq!(r.set_member("sysmon", false), RotationEffect::Switch("pomodoro".into()));
        r.set_paused(true);
        let before = r.remaining();
        assert_eq!(r.tick(), RotationEffect::None);
        assert_eq!(r.remaining(), before);
        r.set_members(&[]);
        assert!(!r.running());
    }
}
