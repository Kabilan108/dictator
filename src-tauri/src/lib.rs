// src-tauri/src/lib.rs

// Declare modules as public so main.rs can potentially access them if needed,
// though direct access is usually minimal.
pub mod audio;
pub mod commands;
pub mod config;
pub mod files;
pub mod whisper;

use audio::AudioRecorder;
use config::{load_config, DictatorConfig};
use files::cleanup_old_recordings;
use std::sync::{Arc, Mutex};
use whisper::WhisperClient;

// Define the state structure (make it public if main.rs needs to reference it,
// but usually it's only managed internally by the builder)
pub struct AppState {
    pub recorder: Arc<Mutex<Option<AudioRecorder>>>,
    pub client: Arc<WhisperClient>,
    pub config: Arc<Mutex<DictatorConfig>>,
}

// Define a serializable error type for commands (make public)
#[derive(serde::Serialize, Debug)]
pub struct CommandError {
    message: String,
}

// Implement From trait to convert various errors into CommandError (make public)
impl<E: std::error::Error> From<E> for CommandError {
    fn from(error: E) -> Self {
        CommandError {
            message: error.to_string(),
        }
    }
}

// Create a public function to run the app
// This function now contains the logic that was previously in main()
#[tokio::main]
pub async fn run() {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("dictator", log::LevelFilter::Debug) // More logs from our app
        .init();
    log::info!("Starting Dictator application setup..."); // Add log

    // Load configuration
    let initial_config = match load_config() {
        Ok(cfg) => {
            log::info!("Configuration loaded successfully.");
            cfg
        }
        Err(e) => {
            log::error!("Failed to load config, using default: {}", e);
            DictatorConfig::default()
        }
    };
    let config_state = Arc::new(Mutex::new(initial_config));

    // Initialize Audio Recorder
    let recorder_state = match AudioRecorder::new() {
        Ok(recorder) => {
            log::info!("Audio recorder initialized successfully.");
            Arc::new(Mutex::new(Some(recorder)))
        }
        Err(e) => {
            log::error!("Failed to initialize audio recorder: {}", e);
            Arc::new(Mutex::new(None)) // Store None if init fails
        }
    };

    // Initialize Whisper Client
    log::info!("Initializing Whisper client...");
    let client_state = Arc::new(WhisperClient::new(config_state.clone()));
    log::info!("Whisper client initialized.");

    // Run cleanup task in background (example)
    tokio::spawn(async {
        log::info!("Spawning background cleanup task.");
        // Run periodically, e.g., every 24 hours
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60 * 60 * 24));
        loop {
            interval.tick().await;
            log::info!("Running periodic cleanup task...");
            if let Err(e) = cleanup_old_recordings() {
                log::error!("Error during cleanup: {}", e);
            }
        }
    });

    log::info!("Building Tauri application...");
    tauri::Builder::default()
        .manage(AppState {
            recorder: recorder_state,
            client: client_state,
            config: config_state,
        })
        .invoke_handler(tauri::generate_handler![
            // Use crate::commands::* to refer to items in the same crate (the library)
            crate::commands::start_recording,
            crate::commands::stop_recording,
            crate::commands::get_settings,
            crate::commands::save_settings,
            crate::commands::list_available_models,
            crate::commands::supports_models_endpoint,
            // Add other commands here
        ])
        .setup(
            #[allow(unused_variables)]
            |app| {
                log::info!("Tauri setup hook running.");
                // You can perform setup tasks here if needed, like creating the main window
                // let main_window = app.get_webview_window("main").unwrap();
                Ok(())
            },
        )
        .run(tauri::generate_context!()) // generate_context! usually works fine here
        .expect("error while running tauri application");

    log::info!("Tauri application has exited."); // Will likely not be reached if .run() blocks indefinitely
}
