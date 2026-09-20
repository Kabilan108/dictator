pub mod config;
pub mod duration;
pub mod logging;
pub mod niri;
pub mod paths;
pub mod process;

pub use config::*;
pub use duration::format_go_duration;
pub use logging::setup_logger;
pub use niri::{discover_niri_sockets, live_niri_socket, niri_socket_candidates};
pub use paths::*;
pub use process::{BoundedOutput, bounded_output};
