//! dictator: a voice typing daemon for Linux.
//!
//! The crate exposes the daemon building blocks so the binary (and tests) can
//! compose them:
//!
//! - [`daemon`]: state machine and orchestration
//! - [`glossary`]: user vocabulary sent to providers that boost terms
//! - [`ipc`]: unix-socket protocol between cli and daemon
//! - [`audio`]: recording and Whisper API transcription
//! - [`typing`]: clipboard + paste simulation
//! - [`notifier`]: D-Bus desktop notifications
//! - [`provider`]: runtime choice of transcription provider
//! - [`visual`]: OSD event stream
//! - [`storage`]: sqlite transcript history
//! - [`utils`]: configuration, paths and logging

pub mod audio;
pub mod daemon;
pub mod glossary;
pub mod ipc;
pub mod notifier;
pub mod provider;
pub mod retry;
pub mod storage;
pub mod typing;
pub mod utils;
pub mod visual;

#[cfg(feature = "gui")]
pub mod gui;
