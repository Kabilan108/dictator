use std::collections::HashMap;
use std::env;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::utils::bounded_output;

const POPUP_TITLE: &str = "Dictator quick controls";
const POPUP_WIDTH: i32 = 340;
const POPUP_HEIGHT: i32 = 510;
const EDGE_GAP: i32 = 16;
const ANCHOR_GAP: i32 = 8;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
const DISCOVERY_INTERVAL: Duration = Duration::from_millis(50);
const MAX_NIRI_OUTPUT_BYTES: usize = 1024 * 1024;
static PLACEMENT_RUNNING: AtomicBool = AtomicBool::new(false);

/// Places the quick-controls window through Niri after GPUI has created it.
///
/// GPUI 0.2 creates Wayland xdg-toplevel windows even for popup window kinds,
/// so Niri initially tiles this window. This helper returns immediately and
/// performs the compositor-specific correction on a bounded background thread.
/// Other compositors keep GPUI's default behavior.
pub fn place_popup(anchor: Option<(i32, i32)>) {
    if env::var_os("NIRI_SOCKET").is_none() {
        return;
    }
    if PLACEMENT_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    if thread::Builder::new()
        .name("dictator-popup-placement".to_string())
        .spawn(move || {
            let _running = PlacementGuard;
            place_popup_with_niri(anchor);
        })
        .is_err()
    {
        PLACEMENT_RUNNING.store(false, Ordering::Release);
    }
}

struct PlacementGuard;

impl Drop for PlacementGuard {
    fn drop(&mut self) {
        PLACEMENT_RUNNING.store(false, Ordering::Release);
    }
}

fn place_popup_with_niri(anchor: Option<(i32, i32)>) {
    let deadline = Instant::now() + DISCOVERY_TIMEOUT;
    let Some(window_id) = find_popup_window(deadline) else {
        return;
    };
    let Some(output) = output_for_anchor(anchor, deadline) else {
        return;
    };
    let (x, y) = popup_position(output, anchor);

    if !niri_action(
        &["move-window-to-floating", "--id", &window_id.to_string()],
        deadline,
    ) {
        return;
    }
    if !niri_action(
        &[
            "set-window-width",
            "--id",
            &window_id.to_string(),
            &POPUP_WIDTH.to_string(),
        ],
        deadline,
    ) {
        return;
    }
    if !niri_action(
        &[
            "set-window-height",
            "--id",
            &window_id.to_string(),
            &POPUP_HEIGHT.to_string(),
        ],
        deadline,
    ) {
        return;
    }
    let _ = niri_action(
        &[
            "move-floating-window",
            "--id",
            &window_id.to_string(),
            "--x",
            &x.to_string(),
            "--y",
            &y.to_string(),
        ],
        deadline,
    );
}

fn find_popup_window(deadline: Instant) -> Option<u64> {
    let pid = std::process::id();
    loop {
        if let Some(windows) = niri_json::<Vec<NiriWindow>>(&["windows"], deadline)
            && let Some(id) = popup_window_id(windows, pid)
        {
            return Some(id);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(DISCOVERY_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn popup_window_id(windows: Vec<NiriWindow>, pid: u32) -> Option<u64> {
    // Window IDs increase over a Niri session. Choosing the largest ID avoids
    // moving a stale duplicate during a rapid popup recreation.
    windows
        .into_iter()
        .filter(|window| window.pid == Some(pid) && window.title.as_deref() == Some(POPUP_TITLE))
        .map(|window| window.id)
        .max()
}

fn output_for_anchor(anchor: Option<(i32, i32)>, deadline: Instant) -> Option<LogicalRect> {
    if let Some(anchor) = anchor
        && let Some(outputs) = niri_json::<HashMap<String, NiriOutput>>(&["outputs"], deadline)
        && let Some(output) = outputs
            .into_values()
            .filter_map(|output| output.logical)
            .find(|output| output.contains(anchor))
    {
        return Some(output);
    }

    niri_json::<NiriOutput>(&["focused-output"], deadline).and_then(|output| output.logical)
}

fn popup_position(output: LogicalRect, anchor: Option<(i32, i32)>) -> (i32, i32) {
    let min_x = output.x;
    let min_y = output.y;
    let max_x = output
        .x
        .saturating_add(output.width)
        .saturating_sub(POPUP_WIDTH);
    let max_y = output
        .y
        .saturating_add(output.height)
        .saturating_sub(POPUP_HEIGHT);

    let (desired_x, desired_y) = if let Some((anchor_x, anchor_y)) = anchor {
        let right = anchor_x.saturating_add(ANCHOR_GAP);
        let left = anchor_x
            .saturating_sub(POPUP_WIDTH)
            .saturating_sub(ANCHOR_GAP);
        let below = anchor_y.saturating_add(ANCHOR_GAP);
        let above = anchor_y
            .saturating_sub(POPUP_HEIGHT)
            .saturating_sub(ANCHOR_GAP);
        (
            if right <= max_x { right } else { left },
            if below <= max_y { below } else { above },
        )
    } else {
        (
            max_x.saturating_sub(EDGE_GAP),
            min_y.saturating_add(EDGE_GAP),
        )
    };

    (
        clamp_to_output(desired_x, min_x, max_x),
        clamp_to_output(desired_y, min_y, max_y),
    )
}

fn clamp_to_output(value: i32, min: i32, max: i32) -> i32 {
    if max < min {
        min
    } else {
        value.clamp(min, max)
    }
}

fn niri_json<T: for<'de> Deserialize<'de>>(request: &[&str], deadline: Instant) -> Option<T> {
    niri_json_with_command(
        Command::new("niri").arg("msg").arg("--json").args(request),
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

fn niri_action(action: &[&str], deadline: Instant) -> bool {
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        return false;
    };
    if remaining.is_zero() {
        return false;
    }
    bounded_output(
        Command::new("niri").arg("msg").arg("action").args(action),
        remaining,
        4096,
    )
    .is_ok_and(|output| !output.timed_out && output.status.success())
}

#[derive(Deserialize)]
struct NiriWindow {
    id: u64,
    title: Option<String>,
    pid: Option<u32>,
}

#[derive(Deserialize)]
struct NiriOutput {
    logical: Option<LogicalRect>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
struct LogicalRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl LogicalRect {
    fn contains(self, (x, y): (i32, i32)) -> bool {
        x >= self.x
            && x < self.x.saturating_add(self.width)
            && y >= self.y
            && y < self.y.saturating_add(self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: LogicalRect = LogicalRect {
        x: 100,
        y: 200,
        width: 1_000,
        height: 800,
    };

    #[test]
    fn default_position_is_top_right_inside_the_output() {
        assert_eq!(popup_position(OUTPUT, None), (744, 216));
    }

    #[test]
    fn anchor_position_flips_at_the_right_and_bottom_edges() {
        assert_eq!(popup_position(OUTPUT, Some((1_050, 950))), (702, 432));
    }

    #[test]
    fn popup_is_clamped_when_the_output_is_smaller_than_it() {
        let small = LogicalRect {
            x: -500,
            y: 40,
            width: 300,
            height: 400,
        };
        assert_eq!(popup_position(small, None), (-500, 40));
    }

    #[test]
    fn window_list_accepts_null_titles() {
        let windows: Vec<NiriWindow> = serde_json::from_str(&format!(
            r#"[
                {{"id": 1, "title": null, "pid": 12}},
                {{"id": 2, "title": "{POPUP_TITLE}", "pid": 12}}
            ]"#
        ))
        .unwrap();
        assert_eq!(windows[0].title, None);
        assert_eq!(windows[1].title.as_deref(), Some(POPUP_TITLE));
        assert_eq!(popup_window_id(windows, 12), Some(2));
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
