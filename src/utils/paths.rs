use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
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
    create_private_dir(&DATA_DIR).map_err(|e| anyhow!("failed to create data dir: {e}"))?;
    create_private_dir(&STATE_DIR).map_err(|e| anyhow!("failed to create state dir: {e}"))?;
    Ok(())
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700).create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// Returns a unique path under `<DATA_DIR>/recordings` for the given start time.
pub fn get_path_to_recording(start_time: DateTime<Local>) -> Result<PathBuf> {
    let dir = DATA_DIR.join("recordings");
    create_private_dir(&dir).map_err(|e| anyhow!("failed to create recording directory: {e:#}"))?;
    let stamp = start_time.format("%m%d%Y-%H%M%S");
    Ok(dir.join(format!("{stamp}-{}.wav", uuid::Uuid::new_v4())))
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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    #[ignore = "subprocess probe"]
    fn private_paths_probe_child() {
        ensure_directories().unwrap();
        assert_eq!(
            std::fs::metadata(&*DATA_DIR).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&*STATE_DIR).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let start = Local::now();
        let first = get_path_to_recording(start).unwrap();
        let second = get_path_to_recording(start).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            std::fs::metadata(first.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn creates_private_unique_paths() {
        let dir = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "utils::paths::tests::private_paths_probe_child",
                "--ignored",
            ])
            .env("XDG_DATA_HOME", dir.path().join("data"))
            .env("XDG_STATE_HOME", dir.path().join("state"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
