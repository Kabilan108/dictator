//! dictator: a voice typing daemon for Linux.
//!
//! The crate exposes the daemon building blocks so the binary (and tests) can
//! compose them:
//!
//! - [`daemon`]: state machine and orchestration
//! - [`ipc`]: unix-socket protocol between cli and daemon
//! - [`audio`]: recording and Whisper API transcription
//! - [`typing`]: clipboard + paste simulation
//! - [`notifier`]: D-Bus desktop notifications
//! - [`visual`]: OSD event stream
//! - [`storage`]: sqlite transcript history
//! - [`utils`]: configuration, paths and logging

pub mod audio;
pub mod daemon;
pub mod ipc;
pub mod notifier;
pub mod storage;
pub mod typing;
pub mod utils;
pub mod visual;
