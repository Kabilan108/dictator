use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::sync::Mutex;
use tracing::{debug, warn};
use zbus::Connection;
use zbus::zvariant::Value;

use crate::ipc::DaemonState;

#[derive(Debug, Clone, Copy)]
pub struct NotificationContent {
    pub title: &'static str,
    pub body: &'static str,
    pub icon: &'static str,
}

const DBUS_SERVICE: &str = "org.freedesktop.Notifications";
const DBUS_PATH: &str = "/org/freedesktop/Notifications";
const DBUS_INTERFACE: &str = "org.freedesktop.Notifications";
const METHOD_NOTIFY: &str = "Notify";
const METHOD_CLOSE: &str = "CloseNotification";
const DBUS_TIMEOUT: Duration = Duration::from_secs(3);

pub fn state_notification(state: DaemonState) -> NotificationContent {
    match state {
        DaemonState::Idle => NotificationContent {
            title: "dictator",
            body: "ready for voice input",
            icon: "audio-input-microphone",
        },
        DaemonState::Recording => NotificationContent {
            title: "dictator",
            body: "Recording audio",
            icon: "media-record",
        },
        DaemonState::Transcribing => NotificationContent {
            title: "dictator",
            body: "transcribing audio",
            icon: "process-working-symbolic",
        },
        DaemonState::Typing => NotificationContent {
            title: "dictator",
            body: "typing text",
            icon: "input-keyboard",
        },
        DaemonState::Error => NotificationContent {
            title: "dictator",
            body: "an error occurred",
            icon: "dialog-error",
        },
    }
}

/// Desktop notification sink.
pub enum Notifier {
    Noop,
    DBus(DBusNotifier),
}

impl Notifier {
    /// Notifications are optional even when enabled in the configuration.
    pub async fn desktop() -> Self {
        match DBusNotifier::new().await {
            Ok(notifier) => Self::DBus(notifier),
            Err(err) => {
                warn!(err = %err, "desktop notifications unavailable; continuing without them");
                Self::Noop
            }
        }
    }

    pub async fn update_state(&self, state: DaemonState) -> Result<()> {
        match self {
            Notifier::Noop => Ok(()),
            Notifier::DBus(n) => n.update_state(state).await,
        }
    }

    pub async fn update(&self, title: &str, body: &str) -> Result<()> {
        match self {
            Notifier::Noop => Ok(()),
            Notifier::DBus(n) => n.update(title, body).await,
        }
    }

    pub async fn close(&self) -> Result<()> {
        match self {
            Notifier::Noop => Ok(()),
            Notifier::DBus(n) => n.close().await,
        }
    }
}

struct DBusState {
    conn: Option<Connection>,
    /// 0 means create a new notification
    notification_id: u32,
}

pub struct DBusNotifier {
    state: Mutex<DBusState>,
}

impl DBusNotifier {
    /// Connects to the session bus and verifies the notification service is
    /// available.
    pub async fn new() -> Result<Self> {
        let conn = tokio::time::timeout(DBUS_TIMEOUT, Connection::session())
            .await
            .context("timed out connecting to D-Bus session bus")?
            .context("failed to connect to D-Bus session bus")?;

        let proxy = tokio::time::timeout(DBUS_TIMEOUT, zbus::fdo::DBusProxy::new(&conn))
            .await
            .context("timed out creating D-Bus proxy")?
            .context("failed to create D-Bus proxy")?;
        let names = tokio::time::timeout(DBUS_TIMEOUT, proxy.list_names())
            .await
            .context("timed out listing D-Bus names")?
            .context("failed to list D-Bus names")?;
        let service_available = names.iter().any(|name| name.as_str() == DBUS_SERVICE);
        drop(proxy);

        if !service_available {
            warn!(
                "notification service not available, D-Bus notification service may not be running"
            );
            bail!("notification service {DBUS_SERVICE} not available");
        }

        debug!("dbus notifier initialized successfully");
        Ok(Self {
            state: Mutex::new(DBusState {
                conn: Some(conn),
                notification_id: 0,
            }),
        })
    }

    /// Updates the notification based on the current daemon state.
    pub async fn update_state(&self, state: DaemonState) -> Result<()> {
        let mut guard = self.state.lock().await;
        let content = state_notification(state);
        debug!(
            title = content.title,
            body = content.body,
            "updating notification state"
        );
        update_notification(&mut guard, content.title, content.body, content.icon).await
    }

    /// Sends a custom notification with the specified title and body.
    pub async fn update(&self, title: &str, body: &str) -> Result<()> {
        let mut guard = self.state.lock().await;
        debug!(title = title, "sending custom notification");
        update_notification(&mut guard, title, body, "").await
    }

    /// Dismisses the current notification and closes the connection.
    pub async fn close(&self) -> Result<()> {
        let mut guard = self.state.lock().await;

        let Some(conn) = guard.conn.take() else {
            warn!("connection already closed");
            return Ok(());
        };

        if guard.notification_id != 0 {
            let result = tokio::time::timeout(
                DBUS_TIMEOUT,
                conn.call_method(
                    Some(DBUS_SERVICE),
                    DBUS_PATH,
                    Some(DBUS_INTERFACE),
                    METHOD_CLOSE,
                    &(guard.notification_id,),
                ),
            )
            .await;
            match result {
                Ok(Ok(_)) => debug!(id = guard.notification_id, "notification closed"),
                Ok(Err(err)) => warn!(err = %err, "failed to close notification"),
                Err(_) => warn!("timed out closing notification"),
            }
            guard.notification_id = 0;
        }

        if tokio::time::timeout(DBUS_TIMEOUT, conn.graceful_shutdown())
            .await
            .is_err()
        {
            warn!("timed out shutting down D-Bus connection");
        }
        debug!("dbus notifier closed");
        Ok(())
    }
}

async fn update_notification(
    state: &mut DBusState,
    title: &str,
    body: &str,
    icon: &str,
) -> Result<()> {
    let Some(conn) = state.conn.as_ref() else {
        bail!("D-Bus connection is closed");
    };

    // Notification parameters according to freedesktop.org spec
    let app_name = "dictator";
    let replace_id = state.notification_id;
    let actions: Vec<&str> = Vec::new();
    let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
    hints.insert("urgency", Value::U8(1)); // Normal urgency
    let timeout: i32 = -1; // Use default timeout

    let reply = tokio::time::timeout(
        DBUS_TIMEOUT,
        conn.call_method(
            Some(DBUS_SERVICE),
            DBUS_PATH,
            Some(DBUS_INTERFACE),
            METHOD_NOTIFY,
            &(
                app_name, replace_id, icon, title, body, actions, hints, timeout,
            ),
        ),
    )
    .await
    .context("timed out sending notification")?
    .context("failed to send notification")?;

    let new_id: u32 = reply
        .body()
        .deserialize()
        .context("failed to get notification ID")?;

    state.notification_id = new_id;
    debug!(id = new_id, "notification sent successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "subprocess probe"]
    async fn missing_notification_bus_probe_child() {
        let notifier = Notifier::desktop().await;
        assert!(matches!(notifier, Notifier::Noop));
        notifier.update_state(DaemonState::Recording).await.unwrap();
        notifier.update("test", "unavailable bus").await.unwrap();
        notifier.close().await.unwrap();
    }

    #[test]
    fn unavailable_notifications_are_optional() {
        let directory = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "notifier::tests::missing_notification_bus_probe_child",
                "--ignored",
            ])
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!(
                    "unix:path={}",
                    directory.path().join("missing-bus").display()
                ),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
