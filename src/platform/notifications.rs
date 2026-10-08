//! Desktop notifications (§10): `BecomeMonitor` on a private session-bus connection.
//!
//! The connection is a raw D-Bus client on the bus socket (SASL EXTERNAL, `Hello`,
//! `BecomeMonitor`). A monitor connection must never send anything once it is a monitor, and a
//! general-purpose client library tends to (match rules, replies to incoming calls), so the few
//! bytes of protocol needed here are written by hand.

use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesktopNotification {
    pub app: String,
    pub summary: String,
    pub body: String,
    /// `app_icon`, or the `desktop-entry` hint when it is empty
    pub icon: String,
}

pub type NotifySink = Arc<dyn Fn(Result<DesktopNotification, String>) + Send + Sync>;

/// Starts monitoring `org.freedesktop.Notifications.Notify` calls. Errors (also later ones)
/// arrive as `Err(text)`. Monitoring stops when the returned guard is dropped.
pub fn spawn_monitor(rt: &tokio::runtime::Handle, on: NotifySink) -> MonitorGuard {
    #[cfg(target_os = "linux")]
    {
        let task = rt.spawn(async move {
            if let Err(e) = linux::run(&on).await {
                on(Err(e));
            }
        });
        MonitorGuard { task: Some(task.abort_handle()) }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = rt;
        on(Err("мониторинг уведомлений недоступен на этой платформе".into()));
        MonitorGuard {}
    }
}

pub struct MonitorGuard {
    #[cfg(target_os = "linux")]
    task: Option<tokio::task::AbortHandle>,
}

impl Drop for MonitorGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// `Notify(app_name s, replaces_id u, app_icon s, summary s, body s, actions as, hints a{sv},
/// expire_timeout i)` → notification.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn notification_from_args(args: &[wire::Value]) -> Option<DesktopNotification> {
    use wire::Value;
    let s = |i: usize| match args.get(i) {
        Some(Value::Str(s)) => Some(s.clone()),
        _ => None,
    };
    let app = s(0)?;
    let _replaces_id = args.get(1);
    let mut icon = s(2)?;
    let summary = s(3)?;
    let body = s(4)?;
    if icon.is_empty()
        && let Some(Value::Array(hints)) = args.get(6)
    {
        for h in hints {
            if let Value::DictEntry(k, v) = h
                && matches!(&**k, Value::Str(k) if k == "desktop-entry")
                && let Value::Variant(v) = &**v
                && let Value::Str(entry) = &**v
            {
                icon = entry.clone();
            }
        }
    }
    Some(DesktopNotification { app, summary, body, icon })
}

/// Minimal D-Bus message (un)marshalling.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod wire {
    pub const METHOD_CALL: u8 = 1;
    pub const METHOD_RETURN: u8 = 2;
    pub const ERROR: u8 = 3;
    /// D-Bus maximum message size
    pub const MAX_MESSAGE: usize = 128 << 20;
    const MAX_DEPTH: u32 = 64;

    #[derive(Clone, Debug, PartialEq)]
    pub enum Value {
        Byte(u8),
        Bool(bool),
        Int(i64),
        UInt(u64),
        Double(f64),
        /// `s`, `o`, `g`
        Str(String),
        /// `ay`
        Bytes(Vec<u8>),
        Array(Vec<Value>),
        Struct(Vec<Value>),
        DictEntry(Box<Value>, Box<Value>),
        Variant(Box<Value>),
    }

    #[derive(Debug, Default)]
    pub struct Message {
        pub kind: u8,
        pub serial: u32,
        pub reply_serial: Option<u32>,
        pub path: String,
        pub interface: String,
        pub member: String,
        pub error_name: String,
        pub signature: String,
        pub body: Vec<Value>,
    }

    /// Total length of the message whose first 16 bytes are `head`.
    pub fn frame_len(head: &[u8; 16]) -> Option<usize> {
        let u32_at = |o: usize| {
            let b = [head[o], head[o + 1], head[o + 2], head[o + 3]];
            match head[0] {
                b'l' => Some(u32::from_le_bytes(b)),
                b'B' => Some(u32::from_be_bytes(b)),
                _ => None,
            }
        };
        let body = u32_at(4)? as usize;
        let fields = u32_at(12)? as usize;
        let len = align(16 + fields, 8).checked_add(body)?;
        (len <= MAX_MESSAGE).then_some(len)
    }

    fn align(n: usize, a: usize) -> usize {
        n.div_ceil(a) * a
    }

    struct Reader<'a> {
        buf: &'a [u8],
        pos: usize,
        le: bool,
    }

    impl<'a> Reader<'a> {
        fn align(&mut self, a: usize) -> Option<()> {
            let p = align(self.pos, a);
            if p > self.buf.len() || self.buf[self.pos..p].iter().any(|&b| b != 0) {
                return None;
            }
            self.pos = p;
            Some(())
        }

        fn take(&mut self, n: usize) -> Option<&'a [u8]> {
            let end = self.pos.checked_add(n)?;
            let s = self.buf.get(self.pos..end)?;
            self.pos = end;
            Some(s)
        }

        fn fixed<const N: usize>(&mut self) -> Option<[u8; N]> {
            self.align(N)?;
            let mut b: [u8; N] = self.take(N)?.try_into().ok()?;
            if !self.le {
                b.reverse();
            }
            Some(b)
        }

        fn u32(&mut self) -> Option<u32> {
            self.fixed::<4>().map(u32::from_le_bytes)
        }

        fn string(&mut self, len: usize) -> Option<String> {
            let s = self.take(len)?;
            (self.take(1)? == [0]).then_some(())?;
            String::from_utf8(s.to_vec()).ok()
        }

        fn value(&mut self, sig: &[u8], depth: u32) -> Option<Value> {
            if depth > MAX_DEPTH {
                return None;
            }
            Some(match sig.first()? {
                b'y' => Value::Byte(self.take(1)?[0]),
                b'b' => Value::Bool(self.u32()? != 0),
                b'n' => Value::Int(i16::from_le_bytes(self.fixed()?) as i64),
                b'q' => Value::UInt(u16::from_le_bytes(self.fixed()?) as u64),
                b'i' => Value::Int(i32::from_le_bytes(self.fixed()?) as i64),
                b'u' | b'h' => Value::UInt(self.u32()? as u64),
                b'x' => Value::Int(i64::from_le_bytes(self.fixed()?)),
                b't' => Value::UInt(u64::from_le_bytes(self.fixed()?)),
                b'd' => Value::Double(f64::from_le_bytes(self.fixed()?)),
                b's' | b'o' => {
                    let n = self.u32()? as usize;
                    Value::Str(self.string(n)?)
                }
                b'g' => {
                    let n = self.take(1)?[0] as usize;
                    Value::Str(self.string(n)?)
                }
                b'v' => {
                    let n = self.take(1)?[0] as usize;
                    let inner = self.string(n)?;
                    let inner = inner.as_bytes();
                    if type_end(inner, 0)? != inner.len() {
                        return None;
                    }
                    Value::Variant(Box::new(self.value(inner, depth + 1)?))
                }
                b'a' => {
                    let n = self.u32()? as usize;
                    let elem = &sig[1..type_end(sig, 1)?];
                    self.align(alignment(elem[0]))?;
                    let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len())?;
                    if elem == b"y" {
                        return Some(Value::Bytes(self.take(n)?.to_vec()));
                    }
                    let mut items = Vec::new();
                    while self.pos < end {
                        items.push(self.value(elem, depth + 1)?);
                    }
                    (self.pos == end).then_some(())?;
                    Value::Array(items)
                }
                b'(' => {
                    self.align(8)?;
                    let end = type_end(sig, 0)? - 1;
                    let mut fields = Vec::new();
                    let mut i = 1;
                    while i < end {
                        let e = type_end(sig, i)?;
                        fields.push(self.value(&sig[i..e], depth + 1)?);
                        i = e;
                    }
                    Value::Struct(fields)
                }
                b'{' => {
                    self.align(8)?;
                    let k = type_end(sig, 1)?;
                    let v = type_end(sig, k)?;
                    let key = self.value(&sig[1..k], depth + 1)?;
                    let val = self.value(&sig[k..v], depth + 1)?;
                    Value::DictEntry(Box::new(key), Box::new(val))
                }
                _ => return None,
            })
        }
    }

    fn alignment(t: u8) -> usize {
        match t {
            b'y' | b'g' | b'v' => 1,
            b'n' | b'q' => 2,
            b'x' | b't' | b'd' | b'(' | b'{' => 8,
            _ => 4,
        }
    }

    /// End index of the single complete type starting at `sig[i]`.
    fn type_end(sig: &[u8], i: usize) -> Option<usize> {
        match *sig.get(i)? {
            b'a' => type_end(sig, i + 1),
            open @ (b'(' | b'{') => {
                let close = if open == b'(' { b')' } else { b'}' };
                let mut j = i + 1;
                while *sig.get(j)? != close {
                    j = type_end(sig, j)?;
                }
                (j > i + 1).then_some(j + 1)
            }
            b'y' | b'b' | b'n' | b'q' | b'i' | b'u' | b'x' | b't' | b'd' | b'h' | b's' | b'o' | b'g' | b'v' => {
                Some(i + 1)
            }
            _ => None,
        }
    }

    pub fn parse(buf: &[u8]) -> Option<Message> {
        let head: &[u8; 16] = buf.get(..16)?.try_into().ok()?;
        if frame_len(head)? != buf.len() {
            return None;
        }
        let mut r = Reader { buf, pos: 4, le: buf[0] == b'l' };
        let mut m = Message { kind: buf[1], ..Default::default() };
        let body_len = r.u32()? as usize;
        m.serial = r.u32()?;
        let Value::Array(fields) = r.value(b"a(yv)", 0)? else { return None };
        for f in fields {
            let Value::Struct(f) = f else { return None };
            let [Value::Byte(code), Value::Variant(v)] = f.as_slice() else { return None };
            match (code, &**v) {
                (1, Value::Str(s)) => m.path = s.clone(),
                (2, Value::Str(s)) => m.interface = s.clone(),
                (3, Value::Str(s)) => m.member = s.clone(),
                (4, Value::Str(s)) => m.error_name = s.clone(),
                (5, Value::UInt(n)) => m.reply_serial = Some(*n as u32),
                (8, Value::Str(s)) => m.signature = s.clone(),
                _ => {}
            }
        }
        r.align(8)?;
        if r.pos + body_len != buf.len() {
            return None;
        }
        let sig = m.signature.clone().into_bytes();
        let mut i = 0;
        while i < sig.len() {
            let e = type_end(&sig, i)?;
            m.body.push(r.value(&sig[i..e], 0)?);
            i = e;
        }
        Some(m)
    }

    #[derive(Default)]
    pub struct Writer {
        pub buf: Vec<u8>,
    }

    impl Writer {
        pub fn pad(&mut self, a: usize) {
            self.buf.resize(align(self.buf.len(), a), 0);
        }

        pub fn u32(&mut self, v: u32) {
            self.pad(4);
            self.buf.extend_from_slice(&v.to_le_bytes());
        }

        pub fn str(&mut self, s: &str) {
            self.u32(s.len() as u32);
            self.buf.extend_from_slice(s.as_bytes());
            self.buf.push(0);
        }

        pub fn sig(&mut self, s: &str) {
            self.buf.push(s.len() as u8);
            self.buf.extend_from_slice(s.as_bytes());
            self.buf.push(0);
        }

        /// `as`
        pub fn str_array(&mut self, items: &[&str]) {
            self.u32(0);
            let len_at = self.buf.len() - 4;
            let start = self.buf.len();
            for s in items {
                self.str(s);
            }
            let n = (self.buf.len() - start) as u32;
            self.buf[len_at..len_at + 4].copy_from_slice(&n.to_le_bytes());
        }
    }

    /// A little-endian method call; `body` must already be marshalled for `sig`.
    pub fn method_call(serial: u32, dest: &str, path: &str, iface: &str, member: &str, sig: &str, body: &[u8]) -> Vec<u8> {
        let mut f = Writer::default();
        f.buf.extend_from_slice(&[b'l', METHOD_CALL, 0, 1]);
        f.u32(body.len() as u32);
        f.u32(serial);
        f.u32(0);
        let start = f.buf.len();
        let mut field = |code: u8, t: &str, v: &str| {
            f.pad(8);
            f.buf.push(code);
            f.sig(t);
            if t == "g" { f.sig(v) } else { f.str(v) }
        };
        field(1, "o", path);
        field(2, "s", iface);
        field(3, "s", member);
        field(6, "s", dest);
        if !sig.is_empty() {
            field(8, "g", sig);
        }
        let n = (f.buf.len() - start) as u32;
        f.buf[12..16].copy_from_slice(&n.to_le_bytes());
        f.pad(8);
        f.buf.extend_from_slice(body);
        f.buf
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{NotifySink, notification_from_args, wire};
    use std::io;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    const RULE: &str = "type='method_call',interface='org.freedesktop.Notifications',member='Notify'";

    pub async fn run(on: &NotifySink) -> Result<(), String> {
        let stream = connect().await.map_err(|e| format!("сессионная шина D-Bus недоступна: {e}"))?;
        let mut conn = BufReader::new(stream);
        auth(&mut conn).await.map_err(|e| format!("авторизация на шине D-Bus: {e}"))?;

        const BUS: (&str, &str, &str) = ("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus");
        let hello = wire::method_call(1, BUS.0, BUS.1, BUS.2, "Hello", "", &[]);
        let mut body = wire::Writer::default();
        body.str_array(&[RULE]);
        body.u32(0);
        let monitor = wire::method_call(2, BUS.0, BUS.1, "org.freedesktop.DBus.Monitoring", "BecomeMonitor", "asu", &body.buf);
        let mut out = hello;
        out.extend_from_slice(&monitor);
        conn.get_mut().write_all(&out).await.map_err(|e| format!("шина D-Bus: {e}"))?;
        // from here on the connection only reads

        let mut monitoring = false;
        loop {
            let msg = read_message(&mut conn).await.map_err(|e| {
                if monitoring { format!("мониторинг уведомлений прерван: {e}") } else { format!("шина D-Bus: {e}") }
            })?;
            let Some(msg) = wire::parse(&msg) else { continue };
            match msg.kind {
                wire::ERROR if matches!(msg.reply_serial, Some(1 | 2)) && !monitoring => {
                    let text = match msg.body.first() {
                        Some(wire::Value::Str(s)) => format!("{}: {s}", msg.error_name),
                        _ => msg.error_name,
                    };
                    return Err(format!("BecomeMonitor не удался: {text}"));
                }
                wire::METHOD_RETURN if msg.reply_serial == Some(2) => monitoring = true,
                wire::METHOD_CALL
                    if msg.interface == "org.freedesktop.Notifications" && msg.member == "Notify" =>
                {
                    if let Some(n) = notification_from_args(&msg.body) {
                        on(Ok(n));
                    }
                }
                _ => {}
            }
        }
    }

    async fn read_message(conn: &mut BufReader<UnixStream>) -> io::Result<Vec<u8>> {
        let mut head = [0u8; 16];
        conn.read_exact(&mut head).await?;
        let len = wire::frame_len(&head)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "повреждённое сообщение"))?;
        let mut msg = vec![0; len];
        msg[..16].copy_from_slice(&head);
        conn.read_exact(&mut msg[16..]).await?;
        Ok(msg)
    }

    async fn auth(conn: &mut BufReader<UnixStream>) -> io::Result<()> {
        let uid = unsafe { libc::geteuid() };
        let hex: String = uid.to_string().bytes().map(|b| format!("{b:02x}")).collect();
        conn.get_mut().write_all(format!("\0AUTH EXTERNAL {hex}\r\n").as_bytes()).await?;
        let mut line = String::new();
        conn.read_line(&mut line).await?;
        if !line.starts_with("OK ") {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, line.trim().to_string()));
        }
        conn.get_mut().write_all(b"BEGIN\r\n").await
    }

    async fn connect() -> io::Result<UnixStream> {
        let mut last = io::Error::new(io::ErrorKind::NotFound, "DBUS_SESSION_BUS_ADDRESS не задан");
        let addresses = match std::env::var("DBUS_SESSION_BUS_ADDRESS") {
            Ok(a) => a,
            Err(_) => match std::env::var("XDG_RUNTIME_DIR") {
                Ok(dir) => format!("unix:path={dir}/bus"),
                Err(_) => return Err(last),
            },
        };
        for addr in addresses.split(';') {
            let Some(params) = addr.strip_prefix("unix:") else { continue };
            for kv in params.split(',') {
                let r = match kv.split_once('=') {
                    Some(("path", p)) => UnixStream::connect(unescape(p)).await,
                    Some(("abstract", name)) => connect_abstract(&unescape(name)),
                    _ => continue,
                };
                match r {
                    Ok(s) => return Ok(s),
                    Err(e) => last = e,
                }
            }
        }
        Err(last)
    }

    fn connect_abstract(name: &str) -> io::Result<UnixStream> {
        use std::os::linux::net::SocketAddrExt;
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
        let s = std::os::unix::net::UnixStream::connect_addr(&addr)?;
        s.set_nonblocking(true)?;
        UnixStream::from_std(s)
    }

    fn unescape(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%'
                && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::wire::{self, Value};
    use super::*;

    /// A `Notify` call captured with `dbus-monitor --binary` from
    /// `notify-send -a "MiniToo probe" -h string:desktop-entry:org.kde.konsole "MiniToo test" "hello <b>world</b> &amp; co"`.
    const CAPTURED: &str = "\
        6c010001c4000000090000009700000001016f001e0000002f6f72672f667265\
        656465736b746f702f4e6f74696669636174696f6e730000020173001d000000\
        6f72672e667265656465736b746f702e4e6f74696669636174696f6e73000000\
        06017300050000003a312e3233000000080167000d73757373736173617b7376\
        7d6900000000000003017300060000004e6f7469667900000701730006000000\
        3a312e35303400000d0000004d696e69546f6f2070726f626500000000000000\
        00000000000000000c0000004d696e69546f6f2074657374000000001b000000\
        68656c6c6f203c623e776f726c643c2f623e2026616d703b20636f0000000000\
        60000000000000000d0000006465736b746f702d656e74727900017300000000\
        0f0000006f72672e6b64652e6b6f6e736f6c6500000000000700000075726765\
        6e637900017900010a00000073656e6465722d70696400017800000000000000\
        f999020000000000ffffffff";

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn parses_captured_notify() {
        let buf = unhex(CAPTURED);
        let head: [u8; 16] = buf[..16].try_into().unwrap();
        assert_eq!(wire::frame_len(&head), Some(buf.len()));
        let m = wire::parse(&buf).unwrap();
        assert_eq!(m.kind, wire::METHOD_CALL);
        assert_eq!(m.path, "/org/freedesktop/Notifications");
        assert_eq!(m.interface, "org.freedesktop.Notifications");
        assert_eq!(m.member, "Notify");
        assert_eq!(m.signature, "susssasa{sv}i");
        assert_eq!(m.body.len(), 8);
        assert_eq!(m.body[7], Value::Int(-1));
        let n = notification_from_args(&m.body).unwrap();
        assert_eq!(
            n,
            DesktopNotification {
                app: "MiniToo probe".into(),
                summary: "MiniToo test".into(),
                body: "hello <b>world</b> &amp; co".into(),
                icon: "org.kde.konsole".into(),
            }
        );
    }

    #[test]
    fn app_icon_wins_over_desktop_entry() {
        let hint = Value::DictEntry(
            Box::new(Value::Str("desktop-entry".into())),
            Box::new(Value::Variant(Box::new(Value::Str("firefox".into())))),
        );
        let s = |v: &str| Value::Str(v.into());
        let mut args = vec![s("Firefox"), Value::UInt(0), s("dialog-information"), s("Hi"), s(""), Value::Array(vec![]), Value::Array(vec![hint]), Value::Int(5000)];
        assert_eq!(notification_from_args(&args).unwrap().icon, "dialog-information");
        args[2] = s("");
        assert_eq!(notification_from_args(&args).unwrap().icon, "firefox");
        assert!(notification_from_args(&args[..3]).is_none());
    }

    #[test]
    fn become_monitor_roundtrip() {
        let mut body = wire::Writer::default();
        body.str_array(&["type='method_call'", "member='Notify'"]);
        body.u32(0);
        let msg = wire::method_call(2, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus.Monitoring", "BecomeMonitor", "asu", &body.buf);
        let m = wire::parse(&msg).unwrap();
        assert_eq!((m.kind, m.serial), (wire::METHOD_CALL, 2));
        assert_eq!(m.member, "BecomeMonitor");
        assert_eq!(m.interface, "org.freedesktop.DBus.Monitoring");
        assert_eq!(
            m.body,
            vec![Value::Array(vec![Value::Str("type='method_call'".into()), Value::Str("member='Notify'".into())]), Value::UInt(0)]
        );
        let hello = wire::method_call(1, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello", "", &[]);
        assert_eq!(wire::parse(&hello).unwrap().member, "Hello");
    }

    #[test]
    fn rejects_garbage() {
        let mut buf = unhex(CAPTURED);
        assert!(wire::parse(&buf[..buf.len() - 1]).is_none());
        // a bogus signature type
        let at = buf.windows(4).position(|w| w == b"suss").unwrap();
        buf[at] = b'Z';
        assert!(wire::parse(&buf).is_none());
    }
}
