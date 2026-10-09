//! Byte transport to the speaker: Bluetooth Classic RFCOMM (SPP).
//!
//! * Linux: an `AF_BLUETOOTH` / `BTPROTO_RFCOMM` socket (no BlueZ library needed), or a bound
//!   `/dev/rfcommN` / any serial device path;
//! * Windows: a Winsock `AF_BTH` socket, or a `COMn` port of a paired SPP device;
//! * macOS: the serial device of the paired speaker (`/dev/cu.Divoom…`).
//!
//! The address setting is either a MAC (`B1:21:81:05:E2:65`) or a device path.

use std::io;

pub trait Transport: Send {
    /// Sends everything (blocking, with the platform send timeout).
    fn send(&mut self, data: &[u8]) -> io::Result<()>;
    /// Waits up to `timeout_ms` for data. `Ok(0)` on timeout; an error when the link is gone.
    fn recv(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<usize>;
}

pub fn is_device_path(address: &str) -> bool {
    address.starts_with('/') || address.to_ascii_uppercase().starts_with("COM")
}

pub fn parse_mac(mac: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = mac.trim().split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        if p.len() != 2 {
            return None;
        }
        out[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(out)
}

pub fn normalize_mac(mac: &str) -> String {
    mac.trim().to_ascii_uppercase().replace('-', ":")
}

/// Opens the link; errors are short human-readable texts (translated where we produce them).
pub fn open(address: &str, channel: u8) -> Result<Box<dyn Transport>, String> {
    if is_device_path(address) {
        return serial::open(address);
    }
    let mac = parse_mac(address).ok_or_else(|| tr!("transport.bad_mac", address = address))?;
    rfcomm::open(mac, channel)
}

// ---------------------------------------------------------------------- Linux RFCOMM

#[cfg(target_os = "linux")]
mod rfcomm {
    use super::Transport;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    const AF_BLUETOOTH: libc::c_int = 31;
    const BTPROTO_RFCOMM: libc::c_int = 3;

    #[repr(C)]
    struct SockaddrRc {
        family: libc::sa_family_t,
        bdaddr: [u8; 6],
        channel: u8,
    }

    pub fn strerror(code: i32) -> String {
        unsafe {
            let p = libc::strerror(code);
            if p.is_null() {
                return tr!("transport.error_code", code = code);
            }
            std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }

    pub struct Rfcomm {
        fd: OwnedFd,
    }

    pub fn open(mac: [u8; 6], channel: u8) -> Result<Box<dyn Transport>, String> {
        unsafe {
            let raw = libc::socket(AF_BLUETOOTH, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, BTPROTO_RFCOMM);
            if raw < 0 {
                return Err(strerror(*libc::__errno_location()));
            }
            let fd = OwnedFd::from_raw_fd(raw);
            let mut bdaddr = mac;
            bdaddr.reverse(); // bdaddr_t is little endian
            let addr = SockaddrRc { family: AF_BLUETOOTH as libc::sa_family_t, bdaddr, channel };
            // non-blocking connect with a 12 s timeout, then blocking with a send timeout
            let flags = libc::fcntl(raw, libc::F_GETFL);
            libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK);
            let rc = libc::connect(
                raw,
                &addr as *const SockaddrRc as *const libc::sockaddr,
                std::mem::size_of::<SockaddrRc>() as libc::socklen_t,
            );
            if rc < 0 {
                let e = *libc::__errno_location();
                if e != libc::EINPROGRESS {
                    return Err(strerror(e));
                }
                let mut p = libc::pollfd { fd: raw, events: libc::POLLOUT, revents: 0 };
                let n = libc::poll(&mut p, 1, 12_000);
                if n == 0 {
                    return Err(tr!("transport.connect_timeout").into());
                }
                let mut soerr: libc::c_int = 0;
                let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
                libc::getsockopt(raw, libc::SOL_SOCKET, libc::SO_ERROR, &mut soerr as *mut _ as *mut libc::c_void, &mut len);
                if soerr != 0 {
                    return Err(strerror(soerr));
                }
            }
            libc::fcntl(raw, libc::F_SETFL, flags & !libc::O_NONBLOCK);
            let tv = libc::timeval { tv_sec: 3, tv_usec: 0 };
            libc::setsockopt(
                raw,
                libc::SOL_SOCKET,
                libc::SO_SNDTIMEO,
                &tv as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            );
            Ok(Box::new(Rfcomm { fd }))
        }
    }

    pub fn poll_recv(fd: i32, buf: &mut [u8], timeout_ms: i32, socket: bool) -> io::Result<usize> {
        unsafe {
            let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            let rc = libc::poll(&mut p, 1, timeout_ms.max(0));
            if rc < 0 {
                let e = io::Error::last_os_error();
                return if e.kind() == io::ErrorKind::Interrupted { Ok(0) } else { Err(e) };
            }
            if rc == 0 {
                return Ok(0);
            }
            if p.revents & (libc::POLLHUP | libc::POLLERR) != 0 && p.revents & libc::POLLIN == 0 {
                return Err(io::Error::new(io::ErrorKind::ConnectionAborted, tr!("transport.link_broken")));
            }
            let n = if socket {
                libc::recv(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0)
            } else {
                libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
            };
            if n <= 0 {
                return Err(io::Error::new(io::ErrorKind::ConnectionAborted, tr!("transport.closed_by_device")));
            }
            Ok(n as usize)
        }
    }

    pub fn send_all(fd: i32, data: &[u8], socket: bool) -> io::Result<()> {
        let mut off = 0;
        while off < data.len() {
            let n = unsafe {
                if socket {
                    libc::send(fd, data[off..].as_ptr() as *const libc::c_void, data.len() - off, libc::MSG_NOSIGNAL)
                } else {
                    libc::write(fd, data[off..].as_ptr() as *const libc::c_void, data.len() - off)
                }
            };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            off += n as usize;
        }
        Ok(())
    }

    impl Transport for Rfcomm {
        fn send(&mut self, data: &[u8]) -> io::Result<()> {
            send_all(self.fd.as_raw_fd(), data, true)
        }
        fn recv(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<usize> {
            poll_recv(self.fd.as_raw_fd(), buf, timeout_ms, true)
        }
    }

    impl Drop for Rfcomm {
        fn drop(&mut self) {
            unsafe {
                libc::shutdown(self.fd.as_raw_fd(), libc::SHUT_RDWR);
            }
        }
    }
}

// ---------------------------------------------------------------------- Windows RFCOMM

#[cfg(windows)]
mod rfcomm {
    use super::Transport;
    use std::io;
    use windows_sys::Win32::Devices::Bluetooth::{AF_BTH, BTHPROTO_RFCOMM, SOCKADDR_BTH};
    use windows_sys::Win32::Networking::WinSock::{
        INVALID_SOCKET, SO_SNDTIMEO, SOCK_STREAM, SOCKADDR, SOCKET, SOL_SOCKET, WSADATA, WSAGetLastError, WSAPOLLFD,
        WSAPoll, WSAStartup, POLLHUP, POLLERR, POLLRDNORM, closesocket, connect, recv, send, setsockopt, socket,
    };

    pub struct Rfcomm {
        s: SOCKET,
    }

    unsafe impl Send for Rfcomm {}

    pub fn open(mac: [u8; 6], channel: u8) -> Result<Box<dyn Transport>, String> {
        unsafe {
            let mut data: WSADATA = std::mem::zeroed();
            WSAStartup(0x0202, &mut data);
            let s = socket(AF_BTH as i32, SOCK_STREAM, BTHPROTO_RFCOMM as i32);
            if s == INVALID_SOCKET {
                return Err(tr!("transport.socket_error", code = WSAGetLastError()));
            }
            let mut addr: SOCKADDR_BTH = std::mem::zeroed();
            addr.addressFamily = AF_BTH;
            addr.btAddr = mac.iter().fold(0u64, |acc, b| (acc << 8) | *b as u64);
            addr.port = channel as u32;
            if connect(s, &addr as *const SOCKADDR_BTH as *const SOCKADDR, std::mem::size_of::<SOCKADDR_BTH>() as i32) != 0 {
                let e = WSAGetLastError();
                closesocket(s);
                return Err(tr!("transport.no_connection_wsa", code = e));
            }
            let timeout: u32 = 3000;
            setsockopt(s, SOL_SOCKET, SO_SNDTIMEO, &timeout as *const u32 as *const u8, 4);
            Ok(Box::new(Rfcomm { s }))
        }
    }

    impl Transport for Rfcomm {
        fn send(&mut self, data: &[u8]) -> io::Result<()> {
            let mut off = 0;
            while off < data.len() {
                let n = unsafe { send(self.s, data[off..].as_ptr(), (data.len() - off) as i32, 0) };
                if n <= 0 {
                    return Err(io::Error::other(format!("WSA {}", unsafe { WSAGetLastError() })));
                }
                off += n as usize;
            }
            Ok(())
        }

        fn recv(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<usize> {
            unsafe {
                let mut p = WSAPOLLFD { fd: self.s, events: POLLRDNORM, revents: 0 };
                let rc = WSAPoll(&mut p, 1, timeout_ms.max(0));
                if rc == 0 {
                    return Ok(0);
                }
                if rc < 0 || (p.revents & (POLLHUP | POLLERR)) != 0 && (p.revents & POLLRDNORM) == 0 {
                    return Err(io::Error::new(io::ErrorKind::ConnectionAborted, tr!("transport.link_broken")));
                }
                let n = recv(self.s, buf.as_mut_ptr(), buf.len() as i32, 0);
                if n <= 0 {
                    return Err(io::Error::new(io::ErrorKind::ConnectionAborted, tr!("transport.closed_by_device")));
                }
                Ok(n as usize)
            }
        }
    }

    impl Drop for Rfcomm {
        fn drop(&mut self) {
            unsafe {
                closesocket(self.s);
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod rfcomm {
    use super::Transport;

    pub fn open(_mac: [u8; 6], _channel: u8) -> Result<Box<dyn Transport>, String> {
        Err(tr!("transport.use_serial_path").into())
    }
}

// ---------------------------------------------------------------------- serial device

#[cfg(target_os = "linux")]
mod serial {
    use super::Transport;
    use std::io;
    use std::os::fd::{AsRawFd, OwnedFd};

    pub struct Serial {
        fd: OwnedFd,
    }

    pub fn open(path: &str) -> Result<Box<dyn Transport>, String> {
        use std::os::unix::fs::OpenOptionsExt;
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY)
            .open(path)
            .map_err(|e| format!("{path}: {e}"))?;
        unsafe {
            // raw mode, no echo
            let fd = f.as_raw_fd();
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut t) == 0 {
                libc::cfmakeraw(&mut t);
                libc::tcsetattr(fd, libc::TCSANOW, &t);
            }
        }
        Ok(Box::new(Serial { fd: f.into() }))
    }

    impl Transport for Serial {
        fn send(&mut self, data: &[u8]) -> io::Result<()> {
            super::rfcomm::send_all(self.fd.as_raw_fd(), data, false)
        }
        fn recv(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<usize> {
            super::rfcomm::poll_recv(self.fd.as_raw_fd(), buf, timeout_ms, false)
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod serial {
    use super::Transport;
    use std::io::{self, Read, Write};
    use std::time::Duration;

    pub struct Serial {
        port: Box<dyn serialport::SerialPort>,
    }

    pub fn open(path: &str) -> Result<Box<dyn Transport>, String> {
        let port = serialport::new(path, 115_200)
            .timeout(Duration::from_millis(10))
            .open()
            .map_err(|e| format!("{path}: {e}"))?;
        Ok(Box::new(Serial { port }))
    }

    impl Transport for Serial {
        fn send(&mut self, data: &[u8]) -> io::Result<()> {
            self.port.write_all(data)?;
            self.port.flush()
        }
        fn recv(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<usize> {
            let _ = self.port.set_timeout(Duration::from_millis(timeout_ms.max(1) as u64));
            match self.port.read(buf) {
                Ok(0) => Err(io::Error::new(io::ErrorKind::ConnectionAborted, tr!("transport.port_closed"))),
                Ok(n) => Ok(n),
                Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(0),
                Err(e) => Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_parsing() {
        assert_eq!(parse_mac("B1:21:81:05:E2:65"), Some([0xb1, 0x21, 0x81, 0x05, 0xe2, 0x65]));
        assert_eq!(parse_mac("b1-21-81-05-e2-65"), Some([0xb1, 0x21, 0x81, 0x05, 0xe2, 0x65]));
        assert_eq!(parse_mac("B1:21:81"), None);
        assert!(is_device_path("/dev/rfcomm0"));
        assert!(is_device_path("COM5"));
        assert!(!is_device_path("B1:21:81:05:E2:65"));
        assert_eq!(normalize_mac(" b1:21:81:05:e2:65 "), "B1:21:81:05:E2:65");
    }
}
