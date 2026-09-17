use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Request from cli to daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    /// unique identifier for request correlation
    pub id: String,
    /// command action: start, stop, toggle, cancel, status
    pub action: String,
    /// optional command arguments
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// request timestamp
    pub timestamp: DateTime<Utc>,
}

/// Daemon's reply to a cli command.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Response {
    /// matches request id
    pub id: String,
    /// whether command succeeded
    pub success: bool,
    /// error message if failed
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// additional response data
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub data: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DaemonState {
    Idle,
    Recording,
    Transcribing,
    Typing,
    Error,
}

impl DaemonState {
    pub fn as_str(self) -> &'static str {
        match self {
            DaemonState::Idle => "idle",
            DaemonState::Recording => "recording",
            DaemonState::Transcribing => "transcribing",
            DaemonState::Typing => "typing",
            DaemonState::Error => "error",
        }
    }
}

impl std::fmt::Display for DaemonState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Data returned by the status command.
#[derive(Debug, Clone)]
pub struct StatusData {
    pub state: DaemonState,
    pub recording_duration: Option<Duration>,
    pub last_error: Option<String>,
    pub uptime: Duration,
}

// Command actions
pub const ACTION_START: &str = "start";
pub const ACTION_STOP: &str = "stop";
pub const ACTION_TOGGLE: &str = "toggle";
pub const ACTION_CANCEL: &str = "cancel";
pub const ACTION_STATUS: &str = "status";

// Socket configuration
pub const SOCKET_PATH: &str = "/tmp/dictator.sock";

// Response data keys
pub const DATA_KEY_STATE: &str = "state";
pub const DATA_KEY_RECORDING_DURATION: &str = "recording_duration";
pub const DATA_KEY_LAST_ERROR: &str = "last_error";
pub const DATA_KEY_UPTIME: &str = "uptime";
pub const DATA_KEY_TEXT: &str = "text";

// Error messages
pub const ERR_INVALID_COMMAND: &str = "invalid command";
pub const ERR_ALREADY_RECORDING: &str = "already recording";
pub const ERR_NOT_RECORDING: &str = "not currently recording";
pub const ERR_RECORDING_FAILED: &str = "recording failed";
pub const ERR_TRANSCRIPTION_FAILED: &str = "transcription failed";
pub const ERR_TYPING_FAILED: &str = "typing failed";
