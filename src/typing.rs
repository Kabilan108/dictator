use std::process::Stdio;
use std::time::Duration;

use anyhow::{Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::utils::config::{PasteShortcut, TypingConfig};

const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(5);
const TERMINATION_TIMEOUT: Duration = Duration::from_secs(1);
const FOCUS_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_FOCUS_RESPONSE: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// xclip + xdotool
    X11,
    /// wl-copy + wtype
    Wayland,
}

/// Types text into the focused window by copying it to the clipboard and
/// simulating the configured paste shortcut.
#[derive(Debug, Clone)]
pub struct Typer {
    backend: Backend,
    config: TypingConfig,
}

/// Detects if the current session is running Wayland.
pub fn is_wayland() -> bool {
    if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland") {
        return true;
    }
    matches!(std::env::var("WAYLAND_DISPLAY"), Ok(v) if !v.is_empty())
}

fn are_installed(cmds: &[&str]) -> bool {
    cmds.iter().all(|cmd| which(cmd))
}

fn which(cmd: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(cmd);
        candidate.is_file()
            && std::fs::metadata(&candidate)
                .map(|m| {
                    use std::os::unix::fs::PermissionsExt;
                    m.permissions().mode() & 0o111 != 0
                })
                .unwrap_or(false)
    })
}

impl Typer {
    /// Creates a Typer based on the current display server.
    pub fn new() -> Result<Self> {
        if is_wayland() {
            if Backend::Wayland.is_available() {
                debug!("using wtype for text input (wayland)");
                return Ok(Self {
                    backend: Backend::Wayland,
                    config: TypingConfig::default(),
                });
            }
            bail!("wayland detected but wtype not available");
        }

        if Backend::X11.is_available() {
            debug!("using xclip/xdotool for text input (x11)");
            return Ok(Self {
                backend: Backend::X11,
                config: TypingConfig::default(),
            });
        }
        bail!("x11 detected but xclip/xdotool not available");
    }

    pub fn with_backend(backend: Backend) -> Self {
        Self {
            backend,
            config: TypingConfig::default(),
        }
    }

    pub fn with_config(mut self, config: TypingConfig) -> Self {
        self.config = config;
        self
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn is_available(&self) -> bool {
        self.backend.is_available()
    }

    pub async fn type_text(&self, cancel: &CancellationToken, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let mut app_id = None;
        if self.backend == Backend::Wayland && !self.config.niri_app_shortcuts.is_empty() {
            for socket in niri_socket_candidates() {
                match focused_app_id(&socket, cancel).await {
                    Ok(app) => {
                        app_id = app;
                        break;
                    }
                    Err(_) if cancel.is_cancelled() => bail!("cancelled"),
                    Err(err) => debug!(socket = %socket.display(), %err, "focus lookup failed"),
                }
            }
            if app_id.is_none() {
                debug!("focused app unavailable; using default paste shortcut");
            }
        }
        let shortcut = self.shortcut_for_app(app_id.as_deref());
        debug!(
            app_id = app_id.as_deref().unwrap_or("unknown"),
            ?shortcut,
            "paste shortcut selected"
        );
        let (copy_cmd, paste_cmd): (&[&str], &[&str]) = match self.backend {
            Backend::X11 => (
                &["xclip", "-selection", "clipboard"],
                match shortcut {
                    PasteShortcut::CtrlV => &["xdotool", "key", "ctrl+v"],
                    PasteShortcut::CtrlShiftV => &["xdotool", "key", "ctrl+shift+v"],
                },
            ),
            Backend::Wayland => (
                &["wl-copy"],
                match shortcut {
                    PasteShortcut::CtrlV => &["wtype", "-M", "ctrl", "-k", "v", "-m", "ctrl"],
                    PasteShortcut::CtrlShiftV => &[
                        "wtype", "-M", "ctrl", "-M", "shift", "-k", "v", "-m", "ctrl", "-m",
                        "shift",
                    ],
                },
            ),
        };
        copy_and_paste(cancel, text, copy_cmd, paste_cmd).await
    }

    fn shortcut_for_app(&self, app_id: Option<&str>) -> PasteShortcut {
        app_id
            .and_then(|app| self.config.niri_app_shortcuts.get(app))
            .copied()
            .unwrap_or(self.config.shortcut)
    }
}

/// Prefers `$NIRI_SOCKET`. The daemon usually runs as a systemd user service
/// without that variable, so fall back to niri's socket naming scheme
/// `$XDG_RUNTIME_DIR/niri.<WAYLAND_DISPLAY>.<pid>.sock`. Stale sockets from an
/// earlier compositor instance are tolerated because callers try each in turn.
fn niri_socket_candidates() -> Vec<std::path::PathBuf> {
    if let Some(socket) = std::env::var_os("NIRI_SOCKET") {
        return vec![std::path::PathBuf::from(socket)];
    }
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
    let display = std::env::var("WAYLAND_DISPLAY").ok();
    discover_niri_sockets(runtime_dir.as_deref(), display.as_deref())
}

fn discover_niri_sockets(
    runtime_dir: Option<&std::path::Path>,
    display: Option<&str>,
) -> Vec<std::path::PathBuf> {
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

/// One read-only Niri IPC request. Never subscribe or retain a connection. Bound
/// both the response size and elapsed time so focus lookup cannot stall typing.
async fn focused_app_id(
    path: &std::path::Path,
    cancel: &CancellationToken,
) -> Result<Option<String>> {
    let query = async {
        let mut stream = UnixStream::connect(path).await?;
        stream.write_all(b"\"FocusedWindow\"\n").await?;
        let mut response = Vec::new();
        BufReader::new(stream.take((MAX_FOCUS_RESPONSE + 1) as u64))
            .read_until(b'\n', &mut response)
            .await?;
        if response.len() > MAX_FOCUS_RESPONSE || response.last() != Some(&b'\n') {
            bail!("invalid focus response size or framing");
        }
        let reply: serde_json::Value = serde_json::from_slice(&response)?;
        let window = reply
            .get("Ok")
            .and_then(|ok| ok.get("FocusedWindow"))
            .ok_or_else(|| anyhow::anyhow!("unexpected focus response"))?;
        Ok(window
            .get("app_id")
            .and_then(|app| app.as_str())
            .map(str::to_owned))
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => bail!("cancelled"),
        result = tokio::time::timeout(FOCUS_TIMEOUT, query) => result?,
    }
}

impl Backend {
    pub fn is_available(self) -> bool {
        match self {
            Backend::X11 => are_installed(&["xclip", "xdotool"]),
            Backend::Wayland => are_installed(&["wl-copy", "wtype"]),
        }
    }
}

async fn run(cancel: &CancellationToken, argv: &[&str], stdin: Option<&str>) -> Result<()> {
    run_with_timeout(cancel, argv, stdin, SUBPROCESS_TIMEOUT).await
}

async fn run_with_timeout(
    cancel: &CancellationToken,
    argv: &[&str],
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<()> {
    if cancel.is_cancelled() {
        bail!("cancelled");
    }

    let mut command = Command::new(argv[0]);
    command.args(&argv[1..]);
    command.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.kill_on_drop(true);

    let mut child = command.spawn()?;
    let deadline = tokio::time::Instant::now() + timeout;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let write_result = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            result = tokio::time::timeout_at(deadline, pipe.write_all(input.as_bytes())) => {
                match result {
                    Ok(result) => Some(result),
                    Err(_) => {
                        terminate(&mut child).await;
                        bail!("timed out after {timeout:?}");
                    }
                }
            }
        };
        drop(pipe);

        match write_result {
            None => {
                terminate(&mut child).await;
                bail!("cancelled");
            }
            Some(Err(err)) => {
                terminate(&mut child).await;
                return Err(err.into());
            }
            Some(Ok(())) => {}
        }
    }

    let status = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            terminate(&mut child).await;
            bail!("cancelled");
        }
        status = tokio::time::timeout_at(deadline, child.wait()) => {
            match status {
                Ok(status) => status?,
                Err(_) => {
                    terminate(&mut child).await;
                    bail!("timed out after {timeout:?}");
                }
            }
        },
    };

    if !status.success() {
        bail!("exit status {}", status.code().unwrap_or(-1));
    }
    Ok(())
}

async fn terminate(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
    let _ = tokio::time::timeout(TERMINATION_TIMEOUT, child.wait()).await;
}

async fn copy_and_paste(
    cancel: &CancellationToken,
    text: &str,
    copy_cmd: &[&str],
    paste_cmd: &[&str],
) -> Result<()> {
    if text.is_empty() {
        debug!("empty text provided, nothing to type");
        return Ok(());
    }

    if let Err(err) = run(cancel, copy_cmd, Some(text)).await {
        if cancel.is_cancelled() {
            debug!("clipboard operation cancelled by context");
            bail!("context canceled");
        }
        bail!("failed to copy text to clipboard: {err}");
    }

    debug!("text copied to clipboard");

    if let Err(err) = run(cancel, paste_cmd, None).await {
        if cancel.is_cancelled() {
            debug!("paste operation cancelled by context");
            bail!("context canceled");
        }
        bail!("failed to paste: {err}");
    }

    debug!("typing successful");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

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

    #[test]
    fn app_override_preserves_default_for_other_windows() {
        let mut config = TypingConfig::default();
        config
            .niri_app_shortcuts
            .insert("com.t3tools.T3Code".into(), PasteShortcut::CtrlV);
        let typer = Typer::with_backend(Backend::Wayland).with_config(config);
        assert_eq!(
            typer.shortcut_for_app(Some("com.t3tools.T3Code")),
            PasteShortcut::CtrlV
        );
        assert_eq!(
            typer.shortcut_for_app(Some("com.mitchellh.ghostty")),
            PasteShortcut::CtrlShiftV
        );
        assert_eq!(typer.shortcut_for_app(None), PasteShortcut::CtrlShiftV);
        assert_eq!(
            typer.shortcut_for_app(Some("com.t3tools.T3Code.other")),
            PasteShortcut::CtrlShiftV
        );
    }

    async fn focus_reply(reply: Vec<u8>) -> Result<Option<String>> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("niri.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 16];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"\"FocusedWindow\"\n");
            let _ = stream.write_all(&reply).await;
        });
        let result = focused_app_id(&path, &CancellationToken::new()).await;
        server.await.unwrap();
        result
    }

    #[tokio::test]
    async fn focus_query_parses_window_and_rejects_invalid_responses() {
        assert_eq!(
            focus_reply(
                b"{\"Ok\":{\"FocusedWindow\":{\"app_id\":\"com.t3tools.T3Code\"}}}\n".to_vec()
            )
            .await
            .unwrap()
            .as_deref(),
            Some("com.t3tools.T3Code")
        );
        assert_eq!(
            focus_reply(b"{\"Ok\":{\"FocusedWindow\":null}}\n".to_vec())
                .await
                .unwrap(),
            None
        );
        assert!(
            focus_reply(b"{\"Err\":\"unavailable\"}\n".to_vec())
                .await
                .is_err()
        );
        assert!(focus_reply(b"invalid\n".to_vec()).await.is_err());
        assert!(
            focus_reply(b"{\"Ok\":{\"FocusedWindow\":null}}".to_vec())
                .await
                .is_err()
        );
        assert!(
            focus_reply(vec![b'x'; MAX_FOCUS_RESPONSE + 1])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn focus_query_is_bounded_and_cancellable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("niri.sock");
        let _listener = tokio::net::UnixListener::bind(&path).unwrap();
        // The listener never replies. Both exit paths must drop the connection.
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            focused_app_id(&path, &CancellationToken::new()),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        let cancel = CancellationToken::new();
        let query = focused_app_id(&path, &cancel);
        let trigger = async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        };
        let (result, _) = tokio::join!(query, trigger);
        assert!(result.unwrap_err().to_string().contains("cancelled"));
    }

    fn stub(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[tokio::test]
    async fn pre_cancelled_command_is_not_spawned() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawned");
        let command = stub(
            dir.path(),
            "must-not-run",
            &format!("touch '{}'", marker.display()),
        );
        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = run_with_timeout(
            &cancel,
            &[command.to_str().unwrap()],
            None,
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("cancelled"));
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn cancellation_interrupts_blocked_stdin_write() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let command = stub(
            dir.path(),
            "ignore-stdin",
            &format!("echo $$ > '{}'\nexec sleep 30", pid_file.display()),
        );
        let cancel = CancellationToken::new();
        let cancel_task = cancel.clone();
        let task_pid_file = pid_file.clone();
        let input = "x".repeat(1024 * 1024);

        let cancellation = tokio::spawn(async move {
            for _ in 0..100 {
                if task_pid_file.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            cancel_task.cancel();
        });
        let err = run_with_timeout(
            &cancel,
            &[command.to_str().unwrap()],
            Some(&input),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        cancellation.await.unwrap();

        assert!(err.to_string().contains("cancelled"));
        let pid = std::fs::read_to_string(pid_file).unwrap();
        assert!(!Path::new("/proc").join(pid.trim()).exists());
    }

    #[tokio::test]
    async fn command_timeout_terminates_child() {
        let dir = tempfile::tempdir().unwrap();
        let command = stub(dir.path(), "too-slow", "exec sleep 30");

        let err = run_with_timeout(
            &CancellationToken::new(),
            &[command.to_str().unwrap()],
            None,
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("timed out"));
    }
}
