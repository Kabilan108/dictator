//! StatusNotifierItem integration. D-Bus work stays off the GPUI thread.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use anyhow::Result;
use ksni::TrayMethods;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenHistory,
    OpenPopup,
    OpenPopupAt(i32, i32),
    ToggleRecording,
    CancelRecording,
    Quit,
}

pub struct TrayHandle {
    updates: tokio::sync::watch::Sender<Option<String>>,
    connected: Arc<AtomicBool>,
}

impl TrayHandle {
    pub fn update(&self, state: &str) {
        self.updates.send_if_modified(|current| {
            if current.as_deref() == Some(state) {
                false
            } else {
                *current = Some(state.to_owned());
                true
            }
        });
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        let _ = self.updates.send(None);
    }
}

pub fn spawn(actions: mpsc::Sender<TrayAction>) -> Result<TrayHandle> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (updates, mut receiver) = tokio::sync::watch::channel(Some("disconnected".to_owned()));
    let connected = Arc::new(AtomicBool::new(false));
    let live = Arc::clone(&connected);
    std::thread::Builder::new().name("dictator-tray".into()).spawn(move || {
        runtime.block_on(async move {
            let tray = DictatorTray { actions, state: "disconnected".into() };
            let handle = match tray.spawn().await {
                Ok(handle) => handle,
                Err(error) => {
                    tracing::warn!(%error, "system tray unavailable; main window remains usable");
                    return;
                }
            };
            live.store(true, Ordering::Relaxed);
            while receiver.changed().await.is_ok() {
                let Some(state) = receiver.borrow_and_update().clone() else { break; };
                if tokio::time::timeout(std::time::Duration::from_secs(2), handle.update(|tray| tray.state = state)).await.is_err() {
                    tracing::warn!("system tray update timed out");
                }
            }
            live.store(false, Ordering::Relaxed);
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle.shutdown()).await;
        });
    })?;
    Ok(TrayHandle { updates, connected })
}

struct DictatorTray {
    actions: mpsc::Sender<TrayAction>,
    state: String,
}

impl ksni::Tray for DictatorTray {
    fn id(&self) -> String {
        "dictator".into()
    }
    fn title(&self) -> String {
        "Dictator".into()
    }
    fn activate(&mut self, x: i32, y: i32) {
        let _ = self.actions.send(TrayAction::OpenPopupAt(x, y));
    }
    fn secondary_activate(&mut self, _: i32, _: i32) {
        let _ = self.actions.send(TrayAction::OpenHistory);
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![cursor_icon(&self.state)]
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        let item = |label: &str, action: TrayAction, enabled: bool| {
            StandardItem {
                label: label.into(),
                enabled,
                activate: Box::new(move |tray: &mut Self| {
                    let _ = tray.actions.send(action);
                }),
                ..Default::default()
            }
            .into()
        };
        vec![
            item("Open Dictator", TrayAction::OpenHistory, true),
            item("Quick recording", TrayAction::OpenPopup, true),
            item(
                if self.state == "recording" {
                    "Stop and transcribe"
                } else {
                    "Start recording"
                },
                TrayAction::ToggleRecording,
                matches!(self.state.as_str(), "idle" | "recording" | "error"),
            ),
            item(
                "Cancel recording",
                TrayAction::CancelRecording,
                matches!(self.state.as_str(), "recording" | "transcribing"),
            ),
            ksni::MenuItem::Separator,
            item("Quit Dictator window", TrayAction::Quit, true),
        ]
    }
}

fn cursor_icon(state: &str) -> ksni::Icon {
    let color = match state {
        "recording" => [255, 243, 139, 168],
        "transcribing" => [255, 137, 180, 250],
        _ => [255, 205, 214, 244],
    };
    let mut data = vec![0; 24 * 24 * 4];
    for (x0, y0, x1, y1) in [
        (3, 9, 5, 15),
        (7, 6, 9, 18),
        (11, 9, 13, 15),
        (18, 4, 20, 20),
        (16, 3, 22, 5),
        (16, 19, 22, 21),
    ] {
        for y in y0..y1 {
            for x in x0..x1 {
                data[(y * 24 + x) * 4..(y * 24 + x + 1) * 4].copy_from_slice(&color);
            }
        }
    }
    ksni::Icon {
        width: 24,
        height: 24,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray;

    #[test]
    fn activation_sends_distinct_popup_and_history_actions() {
        let (tx, rx) = mpsc::channel();
        let mut tray = DictatorTray {
            actions: tx,
            state: "idle".into(),
        };
        tray.activate(100, 25);
        tray.secondary_activate(0, 0);
        assert_eq!(rx.try_recv().unwrap(), TrayAction::OpenPopupAt(100, 25));
        assert_eq!(rx.try_recv().unwrap(), TrayAction::OpenHistory);
        assert_eq!(cursor_icon("recording").data.len(), 24 * 24 * 4);
        assert_ne!(cursor_icon("recording").data, cursor_icon("idle").data);
    }
}
