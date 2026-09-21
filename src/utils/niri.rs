//! Locating the running niri compositor's IPC socket.

use std::path::{Path, PathBuf};

/// Prefers `$NIRI_SOCKET`. The daemon usually runs as a systemd user service
/// without that variable, so fall back to niri's socket naming scheme
/// `$XDG_RUNTIME_DIR/niri.<WAYLAND_DISPLAY>.<pid>.sock`. Stale sockets from an
/// earlier compositor instance are tolerated because callers try each in turn.
/// A `NIRI_SOCKET` that no longer exists (the compositor restarted under a
/// long-lived shell) is skipped in favour of discovery.
pub fn niri_socket_candidates() -> Vec<PathBuf> {
    if let Some(socket) = std::env::var_os("NIRI_SOCKET") {
        let socket = PathBuf::from(socket);
        if socket.exists() {
            return vec![socket];
        }
    }
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let display = std::env::var("WAYLAND_DISPLAY").ok();
    discover_niri_sockets(runtime_dir.as_deref(), display.as_deref())
}

/// The socket to hand `niri msg` through `NIRI_SOCKET`, if any is live.
pub fn live_niri_socket() -> Option<PathBuf> {
    niri_socket_candidates()
        .into_iter()
        .find(|path| std::os::unix::net::UnixStream::connect(path).is_ok())
}

pub fn discover_niri_sockets(runtime_dir: Option<&Path>, display: Option<&str>) -> Vec<PathBuf> {
    let (Some(runtime_dir), Some(display)) = (runtime_dir, display.filter(|d| !d.is_empty()))
    else {
        return Vec::new();
    };
    let prefix = format!("niri.{display}.");
    let Ok(entries) = std::fs::read_dir(runtime_dir) else {
        return Vec::new();
    };
    let mut sockets: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".sock"))
        })
        .collect();
    sockets.sort();
    sockets.reverse();
    sockets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn niri_socket_discovery_matches_the_current_display_only() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "niri.wayland-1.3652.sock",
            "niri.wayland-1.100.sock",
            "niri.wayland-2.777.sock",
            "niri.wayland-1.3652.lock",
        ] {
            std::fs::write(dir.path().join(name), b"").unwrap();
        }
        let found = discover_niri_sockets(Some(dir.path()), Some("wayland-1"));
        assert_eq!(
            found,
            vec![
                dir.path().join("niri.wayland-1.3652.sock"),
                dir.path().join("niri.wayland-1.100.sock"),
            ]
        );
        assert!(discover_niri_sockets(Some(dir.path()), Some("")).is_empty());
        assert!(discover_niri_sockets(None, Some("wayland-1")).is_empty());
        assert!(discover_niri_sockets(Some(dir.path()), None).is_empty());
    }
}
