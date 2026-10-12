//! Status sounds (§11.7): short chiptune cues (four per state to choose from) synthesised in
//! code and played on the speaker's own A2DP sink. The SPP protocol has no "play a sound" command, so the sound goes the same
//! way music does; the volume is the speaker's (AVRCP absolute volume), nothing is scaled here.

use crate::claude::ClaudeState;

pub const RATE: u32 = 48_000;
/// Silence before the cue: a suspended Bluetooth sink needs about a second to start the
/// transport, and whatever is played before that is lost (a 1 s system sound went missing
/// completely, the same sound after 1 s of silence was heard).
pub const LEAD_SECS: f32 = 1.0;
/// Silence after the cue, so the buffered tail is played before the stream closes.
pub const TAIL_SECS: f32 = 0.4;

#[derive(Clone, Copy)]
enum Wave {
    /// pulse with this duty cycle (0.5 = square)
    Pulse(f32),
    Triangle,
}

use Wave::*;

/// A mixing buffer with a cursor. Voices are added on top of each other; `cue` clamps the sum.
#[derive(Default)]
struct Synth {
    buf: Vec<f32>,
    /// where `seq` and `rest` continue, in seconds
    at: f32,
}

impl Synth {
    /// One voice at `at` seconds: frequency gliding from `f0` to `f1`, peak `level`, decay per
    /// second (0 = flat). 3 ms attack and 8 ms release keep the edges from clicking.
    #[allow(clippy::too_many_arguments)]
    fn voice(&mut self, at: f32, f0: f32, f1: f32, secs: f32, wave: Wave, level: f32, decay: f32) {
        let start = (at * RATE as f32) as usize;
        let n = (secs * RATE as f32) as usize;
        if self.buf.len() < start + n {
            self.buf.resize(start + n, 0.0);
        }
        let (attack, release) = (RATE as f32 * 0.003, RATE as f32 * 0.008);
        let mut phase = 0.0f32;
        for i in 0..n {
            let t = i as f32 / RATE as f32;
            phase = (phase + (f0 + (f1 - f0) * i as f32 / n as f32) / RATE as f32).fract();
            let v = match wave {
                Pulse(duty) => {
                    if phase < duty { 1.0 } else { -1.0 }
                }
                Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
            };
            let edge = (i as f32 / attack).min((n - i) as f32 / release).min(1.0);
            self.buf[start + i] += v * level * edge * (-decay * t).exp();
        }
    }

    /// Notes one after another from the cursor: (frequency, seconds); frequency 0 is a rest.
    fn seq(&mut self, notes: &[(f32, f32)], wave: Wave, level: f32, decay: f32) {
        for &(f, secs) in notes {
            if f > 0.0 {
                self.voice(self.at, f, f, secs, wave, level, decay);
            }
            self.at += secs;
        }
        let end = (self.at * RATE as f32) as usize;
        if self.buf.len() < end {
            self.buf.resize(end, 0.0);
        }
    }

    fn glide(&mut self, f0: f32, f1: f32, secs: f32, wave: Wave, level: f32, decay: f32) {
        self.voice(self.at, f0, f1, secs, wave, level, decay);
        self.at += secs;
    }
}

// note frequencies, Hz
const C5: f32 = 523.3;
const E5: f32 = 659.3;
const G5: f32 = 784.0;
const GS5: f32 = 830.6;
const A5: f32 = 880.0;
const B5: f32 = 987.8;
const C6: f32 = 1046.5;
const E6: f32 = 1318.5;
const G6: f32 = 1568.0;
const C7: f32 = 2093.0;

/// Sound variants per state, the default first; `OFF` (no sound) is offered on top of these.
pub fn variants(state: ClaudeState) -> &'static [&'static str] {
    match state {
        ClaudeState::Working => &["blip", "tick", "powerup", "coin"],
        ClaudeState::Alerting => &["arpeggio", "doorbell", "siren", "alarm"],
        ClaudeState::Chilling => &["fanfare", "descend", "chord", "sigh"],
    }
}

/// «no sound for this state»
pub const OFF: &str = "off";

/// The stored choice, or the default for an unknown one.
pub fn resolve(state: ClaudeState, variant: &str) -> &'static str {
    if variant == OFF {
        return OFF;
    }
    variants(state).iter().find(|v| **v == variant).copied().unwrap_or(variants(state)[0])
}

/// Display title in the interface language («Блип», «Без звука», …).
pub fn title(variant: &str) -> &'static str {
    match variant {
        "blip" => tr!("sound.v.blip"),
        "tick" => tr!("sound.v.tick"),
        "powerup" => tr!("sound.v.powerup"),
        "coin" => tr!("sound.v.coin"),
        "arpeggio" => tr!("sound.v.arpeggio"),
        "doorbell" => tr!("sound.v.doorbell"),
        "siren" => tr!("sound.v.siren"),
        "alarm" => tr!("sound.v.alarm"),
        "fanfare" => tr!("sound.v.fanfare"),
        "descend" => tr!("sound.v.descend"),
        "chord" => tr!("sound.v.chord"),
        "sigh" => tr!("sound.v.sigh"),
        _ => tr!("sound.v.off"),
    }
}

/// The cue, without the lead and tail silence; empty for `OFF`. Unknown variant → the default.
/// The alert cues are the loudest, the working ones the quietest.
pub fn cue(state: ClaudeState, variant: &str) -> Vec<f32> {
    let mut s = Synth::default();
    match resolve(state, variant) {
        OFF => return Vec::new(),
        // working: short and quiet
        "blip" => s.seq(&[(C6, 0.06), (0.0, 0.02), (G6, 0.09)], Pulse(0.25), 0.35, 8.0),
        "tick" => s.seq(&[(A5, 0.035), (0.0, 0.07), (E5, 0.035)], Pulse(0.125), 0.4, 0.0),
        "powerup" => s.glide(400.0, 1600.0, 0.2, Pulse(0.25), 0.3, 2.0),
        "coin" => s.seq(&[(B5, 0.06), (E6, 0.22)], Pulse(0.5), 0.3, 6.0),
        // needs you: loud and repeated
        "arpeggio" => {
            let run = [(E5, 0.07), (GS5, 0.07), (B5, 0.07), (E6, 0.07)];
            s.seq(&run, Pulse(0.5), 0.6, 3.0);
            s.seq(&[(0.0, 0.12)], Pulse(0.5), 0.0, 0.0);
            s.seq(&run, Pulse(0.5), 0.6, 3.0);
        }
        "doorbell" => {
            // ding-dong: each note with a quiet octave on top for a bell-like ring
            for (at, f, secs) in [(0.0, E6, 0.32), (0.34, C6, 0.55)] {
                s.voice(at, f, f, secs, Pulse(0.5), 0.45, 5.0);
                s.voice(at, f * 2.0, f * 2.0, secs, Triangle, 0.2, 9.0);
            }
        }
        "siren" => {
            for _ in 0..2 {
                s.glide(660.0, 1320.0, 0.18, Pulse(0.5), 0.55, 0.0);
                s.glide(1320.0, 660.0, 0.18, Pulse(0.5), 0.55, 0.0);
            }
        }
        "alarm" => {
            for round in 0..2 {
                s.seq(&[(C7, 0.06), (0.0, 0.05), (C7, 0.06), (0.0, 0.05), (C7, 0.06)], Pulse(0.5), 0.5, 0.0);
                if round == 0 {
                    s.seq(&[(0.0, 0.18)], Pulse(0.5), 0.0, 0.0);
                }
            }
        }
        // done: soft
        "fanfare" => {
            s.seq(&[(C5, 0.09), (E5, 0.09), (G5, 0.09)], Pulse(0.5), 0.4, 4.0);
            s.seq(&[(C6, 0.32)], Pulse(0.5), 0.4, 6.0);
        }
        "descend" => {
            s.seq(&[(G5, 0.1), (E5, 0.1)], Pulse(0.5), 0.4, 4.0);
            s.seq(&[(C5, 0.35)], Pulse(0.5), 0.4, 5.0);
        }
        "chord" => {
            // a strummed C major chord
            for (i, f) in [C5, E5, G5, C6].into_iter().enumerate() {
                s.voice(i as f32 * 0.04, f, f, 0.7, Pulse(0.5), 0.2, 4.5);
            }
        }
        "sigh" => {
            s.glide(880.0, 440.0, 0.45, Triangle, 0.6, 2.0);
            s.voice(0.0, 880.0, 440.0, 0.45, Pulse(0.5), 0.15, 3.0);
        }
        _ => unreachable!("resolve() returns a known variant"),
    }
    for v in &mut s.buf {
        *v = v.clamp(-1.0, 1.0);
    }
    s.buf
}

/// The cue with the lead and tail silence: what goes to the speaker. Empty for `OFF`.
pub fn padded(state: ClaudeState, variant: &str) -> Vec<f32> {
    let cue = cue(state, variant);
    if cue.is_empty() {
        return cue;
    }
    let lead = (LEAD_SECS * RATE as f32) as usize;
    let tail = (TAIL_SECS * RATE as f32) as usize;
    let mut v = vec![0.0; lead];
    v.extend(cue);
    v.extend(std::iter::repeat_n(0.0, tail));
    v
}

pub type DoneFn = Box<dyn FnOnce(Result<(), String>) + Send>;

/// Plays one sound at a time; a new sound cuts the previous one.
#[derive(Default)]
pub struct Player {
    #[cfg(target_os = "linux")]
    current: Option<crate::platform::pipewire_util::PwThread>,
}

impl Player {
    /// Plays mono `samples` (RATE Hz) on the A2DP sink of the speaker with this MAC. `done`
    /// gets the outcome from the playback thread: an error if the speaker is not connected as
    /// audio or PipeWire is unavailable.
    pub fn play(&mut self, mac: &str, samples: Vec<f32>, done: DoneFn) {
        #[cfg(target_os = "linux")]
        {
            self.current = None; // stops the previous sound
            match linux::start(mac, samples, done) {
                Ok(t) => self.current = Some(t),
                Err(e) => log::warn!("sound: {e}"),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (mac, samples);
            done(Err(tr!("sound.unsupported").into()));
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{DoneFn, RATE};
    use crate::platform::pipewire_util::{self, PwThread};
    use parking_lot::Mutex;
    use pipewire as pw;
    use pw::spa;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    /// `bluez_output.B1_21_81_05_E2_65.1` → true for `B1:21:81:05:E2:65`
    pub fn is_speaker_sink(node_name: &str, media_class: &str, mac: &str) -> bool {
        let prefix = format!("bluez_output.{}.", mac.replace(':', "_"));
        media_class == "Audio/Sink"
            && node_name.len() > prefix.len()
            && node_name[..prefix.len()].eq_ignore_ascii_case(&prefix)
    }

    struct Play {
        samples: Vec<f32>,
        pos: usize,
        draining: bool,
    }

    type Done = Arc<Mutex<Option<DoneFn>>>;

    fn finish(done: &Done, r: Result<(), String>) {
        if let Some(f) = done.lock().take() {
            f(r);
        }
    }

    pub fn start(mac: &str, samples: Vec<f32>, done: DoneFn) -> std::io::Result<PwThread> {
        let done: Done = Arc::new(Mutex::new(Some(done)));
        let (d1, d2) = (done.clone(), done);
        let mac = mac.to_string();
        PwThread::spawn("minitoo-sound", move |ml| setup(ml, mac, samples, d1), move |e| finish(&d2, Err(e)))
    }

    /// Finds the sink in the registry, then (on the core `done` of a sync, when the registry
    /// has been listed) opens a playback stream on it. The loop quits when the stream drains.
    /// If the stream never starts, the idle thread stays until the next sound replaces it.
    fn setup(ml: &pw::main_loop::MainLoopRc, mac: String, samples: Vec<f32>, done: Done) -> Result<pipewire_util::Guard, String> {
        let no_pw = |_| tr!("sound.no_pipewire").to_string();
        let context = pw::context::ContextRc::new(ml, None).map_err(no_pw)?;
        let core = context.connect_rc(None).map_err(no_pw)?;
        let registry = core.get_registry_rc().map_err(no_pw)?;

        let sink: Rc<RefCell<Option<String>>> = Rc::default();
        let found = sink.clone();
        let reg_listener = registry
            .add_listener_local()
            .global(move |g| {
                if g.type_ != pw::types::ObjectType::Node {
                    return;
                }
                let Some(props) = g.props else { return };
                let (name, class) = (props.get("node.name").unwrap_or(""), props.get("media.class").unwrap_or(""));
                if is_speaker_sink(name, class, &mac) {
                    *found.borrow_mut() = Some(name.to_string());
                }
            })
            .register();

        let pending = core.sync(0).map_err(no_pw)?;
        let samples = RefCell::new(Some(samples));
        let stream_slot: Rc<RefCell<Option<pipewire_util::Guard>>> = Rc::default();
        let slot = stream_slot.clone();
        let weak = Rc::new(ml.downgrade());
        let quit = move || {
            if let Some(l) = weak.upgrade() {
                l.quit();
            }
        };
        let core2 = core.clone();
        let core_listener = core
            .add_listener_local()
            .done(move |id, seq| {
                if id != pw::core::PW_ID_CORE || seq != pending {
                    return;
                }
                let Some(samples) = samples.borrow_mut().take() else { return };
                let Some(target) = sink.borrow().clone() else {
                    finish(&done, Err(tr!("sound.no_sink").into()));
                    quit();
                    return;
                };
                match open_stream(&core2, &target, samples, done.clone(), quit.clone()) {
                    Ok(keep) => *slot.borrow_mut() = Some(keep),
                    Err(e) => {
                        finish(&done, Err(e));
                        quit();
                    }
                }
            })
            .register();
        Ok(Box::new((stream_slot, core_listener, reg_listener, registry, core, context)))
    }

    fn open_stream(
        core: &pw::core::CoreRc,
        target: &str,
        samples: Vec<f32>,
        done: Done,
        quit: impl Fn() + Clone + 'static,
    ) -> Result<pipewire_util::Guard, String> {
        let stream = pw::stream::StreamRc::new(
            core.clone(),
            "minitoo-sound",
            pw::properties::properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_CATEGORY => "Playback",
                *pw::keys::MEDIA_ROLE => "Notification",
                *pw::keys::NODE_NAME => "minitoo-sound",
                *pw::keys::APP_NAME => "MiniToo Studio",
                "target.object" => target,
                *pw::keys::NODE_DONT_RECONNECT => "true",
                "node.dont-fallback" => "true",
            },
        )
        .map_err(|e| format!("PipeWire: {e}"))?;

        let (done_err, done_ok, quit_err) = (done.clone(), done, quit.clone());
        let listener = stream
            .add_local_listener_with_user_data(Play { samples, pos: 0, draining: false })
            .state_changed(move |_, _, _old, new| {
                if let pw::stream::StreamState::Error(e) = new {
                    finish(&done_err, Err(tr!("sound.failed", error = e)));
                    quit_err();
                }
            })
            .process(|stream, p| {
                if p.draining {
                    return;
                }
                if let Some(mut buf) = stream.dequeue_buffer() {
                    let Some(data) = buf.datas_mut().first_mut() else { return };
                    const STRIDE: usize = 8; // 2 × f32
                    let mut frames = 0;
                    if let Some(bytes) = data.data() {
                        for frame in bytes.chunks_exact_mut(STRIDE) {
                            let Some(&v) = p.samples.get(p.pos) else { break };
                            let b = v.clamp(-1.0, 1.0).to_le_bytes();
                            frame[..4].copy_from_slice(&b);
                            frame[4..].copy_from_slice(&b);
                            p.pos += 1;
                            frames += 1;
                        }
                    }
                    let chunk = data.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.stride_mut() = STRIDE as _;
                    *chunk.size_mut() = (frames * STRIDE) as _;
                } // the buffer is queued here
                if p.pos >= p.samples.len() {
                    p.draining = true;
                    let _ = stream.flush(true);
                }
            })
            .drained(move |_, _| {
                finish(&done_ok, Ok(()));
                quit();
            })
            .register()
            .map_err(|e| format!("PipeWire: {e}"))?;

        let mut info = spa::param::audio::AudioInfoRaw::new();
        info.set_format(spa::param::audio::AudioFormat::F32LE);
        info.set_rate(RATE);
        info.set_channels(2);
        let bytes = pipewire_util::pod_bytes(spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: info.into(),
        }));
        let pod = spa::pod::Pod::from_bytes(&bytes).ok_or_else(|| tr!("sound.failed", error = "format").to_string())?;
        stream
            .connect(
                spa::utils::Direction::Output,
                None,
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut [pod],
            )
            .map_err(|e| tr!("sound.failed", error = e))?;
        Ok(Box::new((listener, stream)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(v: &[f32]) -> f32 {
        v.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }

    #[test]
    fn every_variant_is_short_audible_and_clickless() {
        for state in ClaudeState::ALL {
            for &v in variants(state) {
                let c = cue(state, v);
                let secs = c.len() as f32 / RATE as f32;
                assert!((0.1..1.2).contains(&secs), "{state:?}/{v}: {secs} s");
                assert!((0.25..=1.0).contains(&peak(&c)), "{state:?}/{v}: peak {}", peak(&c));
                assert_eq!(c.first().copied(), Some(0.0), "{state:?}/{v} starts without a click");
                assert!(c.last().unwrap().abs() < 0.05, "{state:?}/{v} ends without a click");
                assert_ne!(title(v), title(OFF), "{v} has a title");
            }
        }
    }

    #[test]
    fn variants_differ_and_alerts_are_loudest() {
        for state in ClaudeState::ALL {
            let cues: Vec<Vec<f32>> = variants(state).iter().map(|v| cue(state, v)).collect();
            for i in 0..cues.len() {
                for j in i + 1..cues.len() {
                    assert_ne!(cues[i], cues[j], "{state:?}: {i} and {j} are the same");
                }
            }
        }
        let loudest = |s: ClaudeState| variants(s).iter().map(|v| peak(&cue(s, v))).fold(0.0f32, f32::max);
        let quietest = |s: ClaudeState| variants(s).iter().map(|v| peak(&cue(s, v))).fold(1.0f32, f32::min);
        assert!(quietest(ClaudeState::Alerting) > loudest(ClaudeState::Working));
    }

    #[test]
    fn choice_resolves_to_a_known_variant() {
        assert_eq!(resolve(ClaudeState::Working, "coin"), "coin");
        assert_eq!(resolve(ClaudeState::Working, "siren"), "blip"); // another state's sound
        assert_eq!(resolve(ClaudeState::Alerting, ""), "arpeggio");
        assert_eq!(resolve(ClaudeState::Chilling, OFF), OFF);
        assert!(cue(ClaudeState::Chilling, OFF).is_empty());
        assert!(padded(ClaudeState::Chilling, OFF).is_empty());
    }

    #[test]
    fn padding_wraps_the_cue_in_silence() {
        let s = ClaudeState::Chilling;
        let (c, p) = (cue(s, "chord"), padded(s, "chord"));
        let lead = (LEAD_SECS * RATE as f32) as usize;
        assert_eq!(p.len(), lead + c.len() + (TAIL_SECS * RATE as f32) as usize);
        assert!(p[..lead].iter().all(|&x| x == 0.0));
        assert_eq!(&p[lead..lead + c.len()], &c[..]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn finds_the_speaker_sink() {
        let mac = "B1:21:81:05:E2:65";
        assert!(linux::is_speaker_sink("bluez_output.B1_21_81_05_E2_65.1", "Audio/Sink", mac));
        assert!(linux::is_speaker_sink("bluez_output.b1_21_81_05_e2_65.2", "Audio/Sink", "b1:21:81:05:e2:65"));
        assert!(!linux::is_speaker_sink("bluez_input.B1:21:81:05:E2:65", "Audio/Source", mac));
        assert!(!linux::is_speaker_sink("bluez_output.B1_21_81_05_E2_66.1", "Audio/Sink", mac));
        assert!(!linux::is_speaker_sink("bluez_output.B1_21_81_05_E2_65.1", "Audio/Source", mac));
    }
}
