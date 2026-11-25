//! Telling the service manager that started the app that it is ready.
//!
//! A systemd unit of `Type=notify` counts as started only once its main
//! process sends `READY=1` to the socket named in `NOTIFY_SOCKET`; until
//! then `systemctl start` waits and units ordered after it hold back.
//! [`notify_ready`] sends it. The windowed host calls it after presenting
//! the first frame, so a windowed app needs no call of its own; an app that
//! is ready later or without a window calls it itself.
//!
//! Processes inherit `NOTIFY_SOCKET` from whatever started them, and an app
//! launched by a compositor that runs as a notify service would report for
//! the compositor's unit. systemd names the main process in
//! `SYSTEMD_EXEC_PID`, so the message goes out only when that is this
//! process, or when an older systemd sets no such variable.
//!
//! Nothing happens on macOS and Windows: launchd and the Windows service
//! manager have no readiness message for apps.

use std::sync::atomic::{AtomicBool, Ordering};

static DONE: AtomicBool = AtomicBool::new(false);

/// Tells the service manager that started this process that it is ready.
///
/// Only the first call does anything; later ones return `false` at the
/// cost of one atomic load, so a frame loop may call it every frame.
/// Returns whether a message was sent.
pub fn notify_ready() -> bool {
    if DONE.load(Ordering::Relaxed) || DONE.swap(true, Ordering::Relaxed) {
        return false;
    }
    imp::notify(b"READY=1\n")
}

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::OsStr;
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};

    pub(super) fn notify(message: &[u8]) -> bool {
        let Some(socket) = std::env::var_os("NOTIFY_SOCKET") else {
            return false;
        };
        let exec_pid = std::env::var("SYSTEMD_EXEC_PID").ok();
        if !is_main_process(exec_pid.as_deref(), std::process::id()) {
            return false;
        }
        let Some(addr) = address(&socket) else {
            return false;
        };
        let Ok(sender) = UnixDatagram::unbound() else {
            return false;
        };
        sender.send_to_addr(message, &addr).is_ok()
    }

    /// Whether `SYSTEMD_EXEC_PID` names this process or is missing.
    pub(super) fn is_main_process(exec_pid: Option<&str>, pid: u32) -> bool {
        exec_pid.is_none_or(|value| value.trim().parse::<u32>() == Ok(pid))
    }

    /// A path, or an abstract name after `@`. systemd's `vsock:` addresses
    /// for virtual machines are not supported.
    pub(super) fn address(socket: &OsStr) -> Option<SocketAddr> {
        let bytes = socket.as_bytes();
        match bytes.first()? {
            b'@' => SocketAddr::from_abstract_name(&bytes[1..]).ok(),
            b'/' => SocketAddr::from_pathname(socket).ok(),
            _ => None,
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    pub(super) fn notify(_message: &[u8]) -> bool {
        false
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::imp::{address, is_main_process};
    use std::ffi::OsStr;
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::UnixDatagram;

    #[test]
    fn only_the_main_process_reports() {
        assert!(is_main_process(Some("42"), 42));
        assert!(is_main_process(Some("42\n"), 42));
        assert!(!is_main_process(Some("1379"), 42));
        assert!(!is_main_process(Some("not a pid"), 42));
        assert!(is_main_process(None, 42));
    }

    #[test]
    fn reads_path_and_abstract_addresses() {
        let path = address(OsStr::new("/run/user/1000/systemd/notify")).unwrap();
        assert_eq!(
            path.as_pathname(),
            Some(std::path::Path::new("/run/user/1000/systemd/notify"))
        );
        let abstract_name = address(OsStr::new("@/org/freedesktop/systemd1/notify")).unwrap();
        assert_eq!(
            abstract_name.as_abstract_name(),
            Some(&b"/org/freedesktop/systemd1/notify"[..])
        );
        assert!(address(OsStr::new("vsock:2:1234")).is_none());
        assert!(address(OsStr::new("")).is_none());
    }

    #[test]
    fn the_message_reaches_the_socket() {
        let dir = std::env::temp_dir().join(format!("schnellui-notify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notify");
        let _ = std::fs::remove_file(&path);
        let receiver = UnixDatagram::bind(&path).unwrap();
        let addr = address(path.as_os_str()).unwrap();
        let sender = UnixDatagram::unbound().unwrap();
        sender.send_to_addr(b"READY=1\n", &addr).unwrap();
        let mut buf = [0u8; 32];
        let n = receiver.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
