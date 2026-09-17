use std::fs::OpenOptions;
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
    let _ = std::fs::create_dir_all(&*STATE_DIR);
    match OpenOptions::new().create(true).append(true).open(&log_path) {
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
