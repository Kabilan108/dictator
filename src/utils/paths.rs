use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Result, anyhow};
use chrono::{DateTime, Local};

/// `$XDG_DATA_HOME/dictator` (fallback `~/.local/share/dictator`)
pub static DATA_DIR: LazyLock<PathBuf> = LazyLock::new(|| app_dir("XDG_DATA_HOME", "share"));
/// `$XDG_STATE_HOME/dictator` (fallback `~/.local/state/dictator`)
pub static STATE_DIR: LazyLock<PathBuf> = LazyLock::new(|| app_dir("XDG_STATE_HOME", "state"));
/// `$XDG_CONFIG_HOME/dictator` (fallback `~/.config/dictator`)
pub static CONFIG_DIR: LazyLock<PathBuf> = LazyLock::new(config_dir);

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn app_dir(env: &str, fallback: &str) -> PathBuf {
    if let Some(xdg) = std::env::var_os(env).filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("dictator");
    }
    home_dir().join(".local").join(fallback).join("dictator")
}

fn config_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("dictator");
    }
    home_dir().join(".config").join("dictator")
}

pub fn ensure_directories() -> Result<()> {
    std::fs::create_dir_all(&*DATA_DIR).map_err(|e| anyhow!("failed to create data dir: {e}"))?;
    std::fs::create_dir_all(&*STATE_DIR).map_err(|e| anyhow!("failed to create state dir: {e}"))?;
    Ok(())
}

fn create_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(|e| anyhow!("unable to create directory: {e}"))?;
    }
    Ok(())
}

/// Returns `<DATA_DIR>/recordings/MMDDYYYY-HHMMSS.wav` for the given start time.
pub fn get_path_to_recording(start_time: DateTime<Local>) -> Result<PathBuf> {
    let dir = DATA_DIR.join("recordings");
    create_dir(&dir).map_err(|e| anyhow!("failed to create recording directory: {e:#}"))?;
    let stamp = start_time.format("%m%d%Y-%H%M%S");
    Ok(dir.join(format!("{stamp}.wav")))
}

pub fn exit_if_error<T>(result: Result<T>, exit_code: i32) -> T {
    match result {
        Ok(value) => value,
        Err(err) => {
            eprintln!("error: {err:#}");
            std::process::exit(exit_code);
        }
    }
}
