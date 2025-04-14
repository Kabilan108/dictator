// src-tauri/src/files.rs
use directories_next::ProjectDirs;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FileError {
    #[error("Could not find project directories")]
    NoProjectDirs,
    #[error("IO Error: {0}")]
    Io(#[from] io::Error),
    #[error("System time error")]
    TimeError,
}

fn get_project_dirs() -> Result<ProjectDirs, FileError> {
    ProjectDirs::from("com", "YourCompany", "Dictator") // Adjust qualifier, org, app
        .ok_or(FileError::NoProjectDirs)
}

pub fn get_cache_dir() -> Result<PathBuf, FileError> {
    let proj_dirs = get_project_dirs()?;
    let cache_dir = proj_dirs.cache_dir();
    fs::create_dir_all(cache_dir)?;
    Ok(cache_dir.to_path_buf())
}

pub fn get_recordings_dir() -> Result<PathBuf, FileError> {
    let cache_dir = get_cache_dir()?;
    let recordings_dir = cache_dir.join("recordings");
    fs::create_dir_all(&recordings_dir)?;
    Ok(recordings_dir)
}

pub fn create_new_recording_file_path() -> Result<PathBuf, FileError> {
    let dir = get_recordings_dir()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| FileError::TimeError)?
        .as_secs(); // Simple timestamp, format as needed
    let filename = format!("{}.wav", now);
    Ok(dir.join(filename))
}

// TODO: Implement cleanup function for old recordings
pub fn cleanup_old_recordings() -> Result<(), FileError> {
    log::info!("Running cleanup for old recordings (Not Implemented Yet)");
    // 1. Get recordings dir
    // 2. Iterate through files
    // 3. Check file modification time
    // 4. Delete files older than a certain threshold (e.g., 7 days)
    Ok(())
}
