use std::fs::OpenOptions;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;

use tracing::Level;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, registry};

use super::paths::STATE_DIR;

/// Installs the global logger: JSON records (with source location) appended to
/// `<STATE_DIR>/app.log` plus human-readable text on stderr. If the log file
/// cannot be opened a warning is printed and only stderr logging is enabled.
pub fn setup_logger(level: &str) {
    let level = match level {
        "DEBUG" => Level::DEBUG,
        "INFO" => Level::INFO,
        "WARN" => Level::WARN,
        "ERROR" => Level::ERROR,
        other => {
            eprintln!("invalid log level: {other}");
            std::process::exit(1);
        }
    };
    let filter = LevelFilter::from_level(level);

    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_ansi(false);

    let log_path = STATE_DIR.join("app.log");
    match open_private_log(&log_path) {
        Ok(file) => {
            let file_layer = fmt::layer()
                .json()
                .with_writer(Arc::new(file))
                .with_file(true)
                .with_line_number(true)
                .with_target(false);
            registry()
                .with(filter)
                .with(stderr_layer)
                .with(file_layer)
                .init();
        }
        Err(err) => {
            eprintln!("warning: failed to open log file: {err}");
            registry().with(filter).with(stderr_layer).init();
        }
    }
}

fn open_private_log(path: &Path) -> std::io::Result<std::fs::File> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("log path has no parent directory"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::other("log must be a regular file"));
    }
    // mode() applies only at creation. Secure migrated logs through the opened
    // descriptor before a subscriber is allowed to append any new records.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn migrated_log_is_private_and_preserves_existing_records() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        let path = dir.path().join("app.log");
        std::fs::write(&path, b"old record\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mut file = open_private_log(&path).unwrap();
        assert_eq!(
            dir.path().metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        file.write_all(b"new record\n").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"old record\nnew record\n");
    }

    #[test]
    fn log_symlink_does_not_modify_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("unrelated");
        std::fs::write(&target, "leave unchanged").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        let path = dir.path().join("app.log");
        std::os::unix::fs::symlink(&target, &path).unwrap();

        assert!(open_private_log(&path).is_err());
        assert_eq!(
            target.metadata().unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(std::fs::read_to_string(target).unwrap(), "leave unchanged");
    }

    #[test]
    fn fifo_log_is_rejected_without_waiting_for_a_reader() {
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        assert!(open_private_log(&path).is_err());
    }
}
