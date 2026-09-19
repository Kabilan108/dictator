use std::collections::HashMap;
use std::env;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

const POPUP_TITLE: &str = "Dictator quick controls";
const POPUP_WIDTH: i32 = 340;
const POPUP_HEIGHT: i32 = 510;
const EDGE_GAP: i32 = 16;
const ANCHOR_GAP: i32 = 8;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
const DISCOVERY_INTERVAL: Duration = Duration::from_millis(50);

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

    let _ = thread::Builder::new()
        .name("dictator-popup-placement".to_string())
        .spawn(move || place_popup_with_niri(anchor));
}

fn place_popup_with_niri(anchor: Option<(i32, i32)>) {
    let Some(window_id) = find_popup_window() else {
        return;
    };
    let Some(output) = output_for_anchor(anchor) else {
        return;
    };
    let (x, y) = popup_position(output, anchor);

    if !niri_action(&["move-window-to-floating", "--id", &window_id.to_string()]) {
        return;
    }
    if !niri_action(&[
        "set-window-width",
        "--id",
        &window_id.to_string(),
        &POPUP_WIDTH.to_string(),
    ]) {
        return;
    }
    if !niri_action(&[
        "set-window-height",
        "--id",
        &window_id.to_string(),
        &POPUP_HEIGHT.to_string(),
    ]) {
        return;
    }
    let _ = niri_action(&[
        "move-floating-window",
        "--id",
        &window_id.to_string(),
        "--x",
        &x.to_string(),
        "--y",
        &y.to_string(),
    ]);
}

fn find_popup_window() -> Option<u64> {
    let deadline = Instant::now() + DISCOVERY_TIMEOUT;
    let pid = std::process::id();
    loop {
        if let Some(windows) = niri_json::<Vec<NiriWindow>>(&["windows"]) {
            // Window IDs increase over a Niri session. Choosing the largest ID
            // avoids moving a stale duplicate during a rapid popup recreation.
            if let Some(id) = windows
                .into_iter()
                .filter(|window| window.pid == Some(pid) && window.title == POPUP_TITLE)
                .map(|window| window.id)
                .max()
            {
                return Some(id);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(DISCOVERY_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn output_for_anchor(anchor: Option<(i32, i32)>) -> Option<LogicalRect> {
    if let Some(anchor) = anchor
        && let Some(outputs) = niri_json::<HashMap<String, NiriOutput>>(&["outputs"])
        && let Some(output) = outputs
            .into_values()
            .filter_map(|output| output.logical)
            .find(|output| output.contains(anchor))
    {
        return Some(output);
    }

    niri_json::<NiriOutput>(&["focused-output"]).and_then(|output| output.logical)
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

fn niri_json<T: for<'de> Deserialize<'de>>(request: &[&str]) -> Option<T> {
    let output = Command::new("niri")
        .arg("msg")
        .arg("--json")
        .args(request)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn niri_action(action: &[&str]) -> bool {
    Command::new("niri")
        .arg("msg")
        .arg("action")
        .args(action)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[derive(Deserialize)]
struct NiriWindow {
    id: u64,
    title: String,
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
}
