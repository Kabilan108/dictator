use std::process::Stdio;

use anyhow::{Result, bail};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// xclip + xdotool
    X11,
    /// wl-copy + wtype
    Wayland,
}

/// Types text into the focused window by copying it to the clipboard and
/// simulating a ctrl+shift+v paste.
#[derive(Debug, Clone)]
pub struct Typer {
    backend: Backend,
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
                });
            }
            bail!("wayland detected but wtype not available");
        }

        if Backend::X11.is_available() {
            debug!("using xclip/xdotool for text input (x11)");
            return Ok(Self {
                backend: Backend::X11,
            });
        }
        bail!("x11 detected but xclip/xdotool not available");
    }

    pub fn with_backend(backend: Backend) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn is_available(&self) -> bool {
        self.backend.is_available()
    }

    pub async fn type_text(&self, cancel: &CancellationToken, text: &str) -> Result<()> {
        let (copy_cmd, paste_cmd): (&[&str], &[&str]) = match self.backend {
            Backend::X11 => (
                &["xclip", "-selection", "clipboard"],
                &["xdotool", "key", "ctrl+shift+v"],
            ),
            Backend::Wayland => (
                &["wl-copy"],
                &[
                    "wtype", "-M", "ctrl", "-M", "shift", "-k", "v", "-m", "ctrl", "-m", "shift",
                ],
            ),
        };
        copy_and_paste(cancel, text, copy_cmd, paste_cmd).await
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
    let mut command = Command::new(argv[0]);
    command.args(&argv[1..]);
    command.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.kill_on_drop(true);

    let mut child = command.spawn()?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(input.as_bytes()).await?;
        drop(pipe);
    }

    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            bail!("cancelled");
        }
        status = child.wait() => status?,
    };

    if !status.success() {
        bail!("exit status {}", status.code().unwrap_or(-1));
    }
    Ok(())
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
