use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::utils::{bounded_output, live_niri_socket};

const MAIN_TITLE: &str = "Dictator";
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
const DISCOVERY_INTERVAL: Duration = Duration::from_millis(50);
const MAX_NIRI_OUTPUT_BYTES: usize = 1024 * 1024;

/// Sizes the main window through Niri after GPUI has created it.
///
/// GPUI does not request an initial size on Wayland, and Niri hands a new
/// floating toplevel its tiled column size even when a window rule sets a
/// fixed size. Correct it once from a bounded background thread.
pub fn size_main_window(width: i32, height: i32) {
    let Some(socket) = live_niri_socket() else {
        return;
    };
    let _ = thread::Builder::new()
        .name("dictator-main-placement".to_string())
        .spawn(move || {
            let deadline = Instant::now() + DISCOVERY_TIMEOUT;
            let Some(window) = find_window(&socket, MAIN_TITLE, deadline) else {
                return;
            };
            if window.size() == Some((width, height)) {
                return;
            }
            let id = window.id.to_string();
            if !niri_action(
                &socket,
                &["set-window-width", "--id", &id, &width.to_string()],
                deadline,
            ) {
                return;
            }
            let _ = niri_action(
                &socket,
                &["set-window-height", "--id", &id, &height.to_string()],
                deadline,
            );
        });
}

fn find_window(socket: &Path, title: &str, deadline: Instant) -> Option<NiriWindow> {
    let pid = std::process::id();
    loop {
        if let Some(windows) = niri_json::<Vec<NiriWindow>>(socket, &["windows"], deadline)
            && let Some(window) = window_for(windows, pid, title)
        {
            return Some(window);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(DISCOVERY_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn window_for(windows: Vec<NiriWindow>, pid: u32, title: &str) -> Option<NiriWindow> {
    // Window IDs increase over a Niri session. Choosing the largest ID avoids
    // moving a stale duplicate during a rapid window recreation.
    windows
        .into_iter()
        .filter(|window| window.pid == Some(pid) && window.title.as_deref() == Some(title))
        .max_by_key(|window| window.id)
}

fn niri_json<T: for<'de> Deserialize<'de>>(
    socket: &Path,
    request: &[&str],
    deadline: Instant,
) -> Option<T> {
    niri_json_with_command(
        Command::new("niri")
            .env("NIRI_SOCKET", socket)
            .arg("msg")
            .arg("--json")
            .args(request),
        deadline,
    )
}

fn niri_json_with_command<T: for<'de> Deserialize<'de>>(
    command: &mut Command,
    deadline: Instant,
) -> Option<T> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    if remaining.is_zero() {
        return None;
    }
    let output = bounded_output(command, remaining, MAX_NIRI_OUTPUT_BYTES).ok()?;
    if output.timed_out || !output.status.success() || output.stdout_truncated {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn niri_action(socket: &Path, action: &[&str], deadline: Instant) -> bool {
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        return false;
    };
    if remaining.is_zero() {
        return false;
    }
    bounded_output(
        Command::new("niri")
            .env("NIRI_SOCKET", socket)
            .arg("msg")
            .arg("action")
            .args(action),
        remaining,
        4096,
    )
    .is_ok_and(|output| !output.timed_out && output.status.success())
}

#[derive(Debug, Deserialize)]
struct NiriWindow {
    id: u64,
    title: Option<String>,
    pid: Option<u32>,
    #[serde(default)]
    layout: NiriLayout,
}

#[derive(Debug, Default, Deserialize)]
struct NiriLayout {
    window_size: Option<(i32, i32)>,
}

impl NiriWindow {
    fn size(&self) -> Option<(i32, i32)> {
        self.layout.window_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_list_accepts_null_titles() {
        let windows: Vec<NiriWindow> = serde_json::from_str(&format!(
            r#"[
                {{"id": 1, "title": null, "pid": 12}},
                {{"id": 2, "title": "{MAIN_TITLE}", "pid": 12}}
            ]"#
        ))
        .unwrap();
        assert_eq!(windows[0].title, None);
        assert_eq!(windows[1].title.as_deref(), Some(MAIN_TITLE));
        let window = window_for(windows, 12, MAIN_TITLE).unwrap();
        assert_eq!(window.id, 2);
        assert_eq!(window.size(), None);
    }

    #[test]
    fn window_size_is_read_from_niri() {
        let windows: Vec<NiriWindow> = serde_json::from_str(&format!(
            r#"[{{"id": 7, "title": "{MAIN_TITLE}", "pid": 3, "is_floating": true,
                 "layout": {{"window_size": [1120, 700], "tile_size": [1120, 700]}}}}]"#
        ))
        .unwrap();
        let window = window_for(windows, 3, MAIN_TITLE).unwrap();
        assert_eq!(window.size(), Some((1120, 700)));
    }

    #[test]
    fn hung_niri_query_stops_at_the_placement_deadline() {
        let started = Instant::now();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec sleep 10"]);

        let result = niri_json_with_command::<serde_json::Value>(
            &mut command,
            started + Duration::from_millis(100),
        );

        assert!(result.is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
