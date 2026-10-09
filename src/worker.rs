//! Device channel (§5): a thread that owns the link — connect / reconnect with back-off,
//! ordered command queue, "last one wins" media, keepalive, incoming frame parsing.
//! zstd encoding of media runs here, not on the UI or controller thread.

use crate::frame::Frame;
use crate::protocol::{self, ColorDepth, FrameParser, Incoming};
use crate::transport::{self, Transport};
use parking_lot::{Condvar, Mutex};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

const REQUEST_MARK: [u8; 3] = [0x04, 0x8b, 0x55];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Disconnected = 0,
    Connecting = 1,
    Connected = 2,
}

#[derive(Clone, Debug)]
pub struct MediaJob {
    pub id: u64,
    pub frames: Vec<Frame>,
    pub speed: u32,
    pub level: i32,
    pub depth: ColorDepth,
    pub streaming: bool,
}

#[derive(Clone, Debug)]
pub enum WorkerEvent {
    State(LinkState),
    Log(String),
    /// `ok = false`: replaced before it started or dropped with the link
    MediaSent { id: u64, bytes: usize, frames: usize, ms: u64, ok: bool },
    Frame(Incoming),
}

pub type WorkerSink = Arc<dyn Fn(WorkerEvent) + Send + Sync>;
pub type Opener = Arc<dyn Fn(&str, u8) -> Result<Box<dyn Transport>, String> + Send + Sync>;

#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub address: String,
    pub channel: u8,
    pub chunk_delay_ms: u64,
    /// seconds, 0 = off
    pub keepalive: u64,
}

struct Shared {
    config: WorkerConfig,
    want: bool,
    retry_now: bool,
    reconnect_now: bool,
    quit: bool,
    pending_media: Option<MediaJob>,
    commands: VecDeque<(u8, Vec<u8>)>,
    next_id: u64,
}

pub struct DeviceWorker {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    sink: WorkerSink,
    opener: Opener,
}

impl DeviceWorker {
    pub fn new(config: WorkerConfig, sink: WorkerSink) -> Self {
        Self::with_opener(config, sink, Arc::new(|a: &str, c: u8| transport::open(a, c)))
    }

    /// With a custom transport factory (tests, simulators).
    pub fn with_opener(config: WorkerConfig, sink: WorkerSink, opener: Opener) -> Self {
        let shared = Shared {
            config,
            want: false,
            retry_now: false,
            reconnect_now: false,
            quit: false,
            pending_media: None,
            commands: VecDeque::new(),
            next_id: 1,
        };
        DeviceWorker { shared: Arc::new((Mutex::new(shared), Condvar::new())), thread: Mutex::new(None), sink, opener }
    }

    pub fn start(&self) {
        let mut t = self.thread.lock();
        if t.is_some() {
            return;
        }
        let shared = self.shared.clone();
        let sink = self.sink.clone();
        let opener = self.opener.clone();
        *t = Some(
            std::thread::Builder::new()
                .name("device-worker".into())
                .spawn(move || Loop::new(shared, sink, opener).run())
                .expect("worker thread"),
        );
    }

    pub fn is_running(&self) -> bool {
        self.thread.lock().is_some()
    }

    /// A changed address or channel drops the link and reconnects.
    pub fn configure(&self, config: WorkerConfig) {
        let (m, cv) = &*self.shared;
        let mut s = m.lock();
        if s.config.address != config.address || s.config.channel != config.channel {
            s.reconnect_now = true;
        }
        s.config = config;
        cv.notify_all();
    }

    /// `true` skips the retry pause.
    pub fn set_want_connected(&self, want: bool) {
        let (m, cv) = &*self.shared;
        let mut s = m.lock();
        s.want = want;
        if want {
            s.retry_now = true;
        }
        cv.notify_all();
    }

    /// "Last one wins": a job not yet started is replaced (and reported with `ok = false`).
    /// Submitting also asks for the link.
    pub fn submit_media(&self, mut job: MediaJob) -> u64 {
        let (m, cv) = &*self.shared;
        let (id, superseded) = {
            let mut s = m.lock();
            job.id = s.next_id;
            s.next_id += 1;
            let id = job.id;
            let superseded = s.pending_media.replace(job).map(|j| j.id);
            s.want = true;
            cv.notify_all();
            (id, superseded)
        };
        if let Some(old) = superseded {
            (self.sink)(WorkerEvent::MediaSent { id: old, bytes: 0, frames: 0, ms: 0, ok: false });
        }
        id
    }

    pub fn submit_command(&self, cmd: u8, args: Vec<u8>) {
        if protocol::FORBIDDEN_COMMANDS.contains(&cmd) {
            (self.sink)(WorkerEvent::Log(tr!("worker.command_forbidden", cmd = format!("{cmd:#04x}"))));
            return;
        }
        let (m, cv) = &*self.shared;
        let mut s = m.lock();
        s.commands.push_back((cmd, args));
        s.want = true;
        cv.notify_all();
    }

    pub fn shutdown(&self) {
        {
            let (m, cv) = &*self.shared;
            m.lock().quit = true;
            cv.notify_all();
        }
        if let Some(t) = self.thread.lock().take() {
            let _ = t.join();
        }
    }
}

impl Drop for DeviceWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Loop {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    sink: WorkerSink,
    link: Option<Box<dyn Transport>>,
    parser: FrameParser,
    /// raw bytes since the start of a media transfer, scanned for flow-control requests
    raw: Vec<u8>,
    state: LinkState,
    opener: Opener,
}

impl Loop {
    fn new(shared: Arc<(Mutex<Shared>, Condvar)>, sink: WorkerSink, opener: Opener) -> Self {
        Loop { shared, sink, link: None, parser: FrameParser::new(), raw: Vec::new(), state: LinkState::Disconnected, opener }
    }

    fn log(&self, s: impl Into<String>) {
        (self.sink)(WorkerEvent::Log(s.into()));
    }

    fn set_state(&mut self, st: LinkState) {
        if self.state != st {
            self.state = st;
            (self.sink)(WorkerEvent::State(st));
        }
    }

    fn close(&mut self) {
        self.link = None;
        self.raw.clear();
        self.parser = FrameParser::new();
    }

    /// Reads what is available within `timeout_ms`; -1 = link lost.
    fn receive(&mut self, timeout_ms: i32) -> isize {
        let Some(link) = self.link.as_mut() else { return -1 };
        let mut buf = [0u8; 1024];
        match link.recv(&mut buf, timeout_ms) {
            Ok(0) => 0,
            Ok(n) => {
                self.raw.extend_from_slice(&buf[..n]);
                if self.raw.len() > 65_536 {
                    let cut = self.raw.len() - 4096;
                    self.raw.drain(..cut);
                }
                self.parser.push(&buf[..n]);
                while let Some(f) = self.parser.next_frame() {
                    (self.sink)(WorkerEvent::Frame(f));
                }
                n as isize
            }
            Err(_) => -1,
        }
    }

    fn send_command(&mut self, cmd: u8, args: &[u8]) -> bool {
        let Some(link) = self.link.as_mut() else { return false };
        match link.send(&protocol::make_message(cmd, args)) {
            Ok(()) => true,
            Err(e) => {
                self.log(tr!("worker.send_error", error = e));
                false
            }
        }
    }

    /// `04 8b 55 <flag> [seq LE16]` in the raw stream.
    fn take_request(&mut self) -> Option<(u8, usize)> {
        let at = self.raw.windows(3).position(|w| w == REQUEST_MARK)?;
        if self.raw.len() < at + 4 {
            return None;
        }
        let flag = self.raw[at + 3];
        if flag != 0x01 {
            self.raw.drain(..at + 4);
            return Some((flag, 0));
        }
        if self.raw.len() < at + 6 {
            return None;
        }
        let index = u16::from_le_bytes([self.raw[at + 4], self.raw[at + 5]]) as usize;
        self.raw.drain(..at + 6);
        Some((flag, index))
    }

    /// `Err` = link lost. `Ok((bytes, frames))`.
    fn send_media(&mut self, job: &MediaJob, chunk_delay: u64) -> Result<(usize, usize), ()> {
        let media = protocol::encode_media(&job.frames, job.speed, job.level, job.depth);
        if media.payload.is_empty() {
            self.log(tr!("worker.encode_failed"));
            return Ok((0, 0));
        }
        let packets = protocol::media_packets(&media.payload);
        self.raw.clear();
        if !self.send_command(0x8b, &packets[0]) {
            return Err(());
        }
        // wait for "give me the data" (04 8b 55 00)
        let t = Instant::now();
        let mut asked = false;
        while t.elapsed() < Duration::from_millis(2000) {
            if self.receive(100) < 0 {
                return Err(());
            }
            if self.take_request().is_some() {
                asked = true;
                break;
            }
        }
        if !asked {
            self.log(tr!("worker.no_request"));
        }
        let mut budget = packets.len();
        let mut resent = 0;
        let serve = |me: &mut Loop, budget: &mut usize, resent: &mut usize| -> bool {
            while *budget > 0 {
                let Some((flag, index)) = me.take_request() else { break };
                if flag == 0x01 && index + 1 < packets.len() {
                    if !me.send_command(0x8b, &packets[1 + index]) {
                        return false;
                    }
                    *budget -= 1;
                    *resent += 1;
                }
            }
            true
        };
        for p in &packets[1..] {
            if !self.send_command(0x8b, p) {
                return Err(());
            }
            if chunk_delay > 0 {
                std::thread::sleep(Duration::from_millis(chunk_delay));
            }
            if self.receive(0) < 0 || !serve(self, &mut budget, &mut resent) {
                return Err(());
            }
        }
        // the device may still ask for single chunks shortly after the stream
        let tail = Duration::from_millis(if job.streaming { 40 } else { 150 });
        let t = Instant::now();
        while budget > 0 && t.elapsed() < tail {
            let n = self.receive(40);
            if n < 0 {
                return Err(());
            }
            if n > 0 && !serve(self, &mut budget, &mut resent) {
                return Err(());
            }
        }
        self.raw.clear();
        if resent > 0 {
            self.log(tr!("worker.resent", resent = resent, total = packets.len() - 1));
        }
        Ok((media.payload.len(), media.frames))
    }

    fn run(mut self) {
        let clock = Instant::now();
        let mut last_activity = Instant::now();
        let mut next_attempt = Duration::ZERO;
        let mut backoff = 0u64;
        loop {
            let mut media: Option<MediaJob> = None;
            let mut commands: VecDeque<(u8, Vec<u8>)> = VecDeque::new();
            let (want, config) = {
                let (m, cv) = &*self.shared;
                let mut s = m.lock();
                if s.quit {
                    break;
                }
                if s.retry_now {
                    s.retry_now = false;
                    next_attempt = Duration::ZERO;
                }
                if s.reconnect_now {
                    s.reconnect_now = false;
                    next_attempt = Duration::ZERO;
                    if self.link.is_some() && s.want {
                        drop(s);
                        self.close();
                        self.set_state(LinkState::Disconnected);
                        continue;
                    }
                }
                let connected = self.link.is_some();
                let have_work = s.pending_media.is_some() || !s.commands.is_empty();
                if connected && have_work {
                    std::mem::swap(&mut commands, &mut s.commands);
                    media = s.pending_media.take();
                } else {
                    let keepalive = s.config.keepalive;
                    let wait = if s.want && !connected {
                        next_attempt.saturating_sub(clock.elapsed())
                    } else if connected && keepalive > 0 {
                        Duration::from_secs(keepalive).saturating_sub(last_activity.elapsed()).min(Duration::from_secs(1))
                    } else {
                        Duration::from_secs(1)
                    };
                    if !wait.is_zero() {
                        cv.wait_for(&mut s, wait);
                    }
                    if s.quit {
                        break;
                    }
                }
                (s.want, s.config.clone())
            };

            if !want {
                if self.link.is_some() {
                    self.close();
                    self.log(tr!("worker.disconnected"));
                }
                self.set_state(LinkState::Disconnected);
                continue;
            }

            if self.link.is_none() {
                if clock.elapsed() < next_attempt {
                    continue;
                }
                self.set_state(LinkState::Connecting);
                match (self.opener)(&config.address, config.channel) {
                    Err(error) => {
                        backoff = if backoff == 0 { 2 } else { (backoff * 2).min(30) };
                        next_attempt = clock.elapsed() + Duration::from_secs(backoff);
                        self.log(tr!("worker.no_link", error = error, secs = backoff));
                        self.set_state(LinkState::Disconnected);
                    }
                    Ok(link) => {
                        backoff = 0;
                        self.link = Some(link);
                        self.parser = FrameParser::new();
                        std::thread::sleep(Duration::from_millis(300));
                        self.receive(100);
                        self.raw.clear();
                        last_activity = Instant::now();
                        self.set_state(LinkState::Connected);
                        self.log(tr!("worker.connected"));
                    }
                }
                continue;
            }

            // drain unsolicited data and notice a dropped link while idle
            let mut lost = self.receive(0) < 0;
            let mut sent = false;
            for (cmd, args) in &commands {
                if lost {
                    break;
                }
                if !self.send_command(*cmd, args) {
                    lost = true;
                }
                sent = true;
                std::thread::sleep(Duration::from_millis(20));
            }
            if !lost {
                if let Some(job) = media.take() {
                    sent = true;
                    let t = Instant::now();
                    match self.send_media(&job, config.chunk_delay_ms) {
                        Ok((bytes, frames)) => (self.sink)(WorkerEvent::MediaSent {
                            id: job.id,
                            bytes,
                            frames,
                            ms: t.elapsed().as_millis() as u64,
                            ok: true,
                        }),
                        Err(()) => {
                            lost = true;
                            media = Some(job);
                        }
                    }
                }
            }
            if !lost && !sent && config.keepalive > 0 && last_activity.elapsed() >= Duration::from_secs(config.keepalive) {
                // cheap GETs the MiniToo answers: volume and the 0x06 level byte
                if !self.send_command(0x09, &[]) || !self.send_command(0x06, &[]) {
                    lost = true;
                }
                sent = true;
            }
            if sent {
                last_activity = Instant::now();
            }
            if lost {
                self.close();
                self.set_state(LinkState::Disconnected);
                self.log(tr!("worker.link_lost"));
                next_attempt = Duration::ZERO;
                if let Some(job) = media {
                    let dropped = {
                        let (m, _) = &*self.shared;
                        let mut s = m.lock();
                        // put the unfinished job back unless something newer is waiting
                        if s.pending_media.is_none() {
                            s.pending_media = Some(job);
                            None
                        } else {
                            Some(job.id)
                        }
                    };
                    if let Some(id) = dropped {
                        (self.sink)(WorkerEvent::MediaSent { id, bytes: 0, frames: 0, ms: 0, ok: false });
                    }
                }
            }
        }
        self.close();
        self.set_state(LinkState::Disconnected);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    /// Simulated speaker: answers the start packet with "give me data", asks once for chunk 1
    /// again, records every 0x8b packet.
    struct FakeDevice {
        log: Arc<Mutex<Vec<Vec<u8>>>>,
        out: VecDeque<u8>,
        asked_resend: bool,
    }

    impl Transport for FakeDevice {
        fn send(&mut self, data: &[u8]) -> io::Result<()> {
            let mut p = FrameParser::new();
            p.push(data);
            while let Some(f) = p.next_frame() {
                if f.cmd == 0x8b {
                    if f.data.first() == Some(&0x00) {
                        self.out.extend(protocol::make_message(0x04, &[0x8b, 0x55, 0x00]));
                    } else if !self.asked_resend && f.data.len() > 6 && f.data[5] == 1 {
                        self.asked_resend = true;
                        self.out.extend(protocol::make_message(0x04, &[0x8b, 0x55, 0x01, 0x00, 0x00]));
                    }
                }
                self.log.lock().push(f.raw);
            }
            Ok(())
        }
        fn recv(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<usize> {
            if self.out.is_empty() {
                std::thread::sleep(Duration::from_millis(timeout_ms.clamp(0, 5) as u64));
                return Ok(0);
            }
            let n = buf.len().min(self.out.len());
            for b in buf.iter_mut().take(n) {
                *b = self.out.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    #[test]
    fn media_flow_control_with_resend_and_last_wins() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let sink: WorkerSink = Arc::new(move |e| ev.lock().push(e));
        let l2 = log.clone();
        let opener: Opener = Arc::new(move |_: &str, _: u8| {
            Ok(Box::new(FakeDevice { log: l2.clone(), out: VecDeque::new(), asked_resend: false }) as Box<dyn Transport>)
        });
        let cfg = WorkerConfig { address: "00:00:00:00:00:01".into(), channel: 1, chunk_delay_ms: 0, keepalive: 0 };
        let w = DeviceWorker::with_opener(cfg, sink, opener);
        // queued before the thread runs: the first job is superseded
        let mut seed = 7u32;
        let noise: Vec<u8> = (0..crate::frame::FRAME_BYTES)
            .map(|_| {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                (seed >> 24) as u8
            })
            .collect();
        let first = w.submit_media(MediaJob { id: 0, frames: vec![Frame::black()], speed: 1000, level: 3, depth: ColorDepth::Full, streaming: false });
        let second = w.submit_media(MediaJob { id: 0, frames: vec![Frame::from_rgb(noise)], speed: 1000, level: 1, depth: ColorDepth::Full, streaming: false });
        w.start();
        let t = Instant::now();
        loop {
            let done = events.lock().iter().any(|e| matches!(e, WorkerEvent::MediaSent { id, ok: true, .. } if *id == second));
            if done || t.elapsed() > Duration::from_secs(10) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        w.shutdown();
        let ev = events.lock();
        assert!(ev.iter().any(|e| matches!(e, WorkerEvent::MediaSent { id, ok: false, .. } if *id == first)));
        let (bytes, frames) = ev
            .iter()
            .find_map(|e| match e {
                WorkerEvent::MediaSent { id, ok: true, bytes, frames, .. } if *id == second => Some((*bytes, *frames)),
                _ => None,
            })
            .expect("second job sent");
        assert_eq!(frames, 1);
        let packets: Vec<Vec<u8>> = log.lock().clone();
        let chunks = bytes.div_ceil(protocol::CHUNK_SIZE);
        // start packet + every chunk + one resent chunk
        assert_eq!(packets.len(), 1 + chunks + 1);
        assert!(ev.iter().any(|e| matches!(e, WorkerEvent::Log(s) if s.contains("повторно 1"))));
        assert!(ev.iter().any(|e| matches!(e, WorkerEvent::State(LinkState::Connected))));
    }
}
