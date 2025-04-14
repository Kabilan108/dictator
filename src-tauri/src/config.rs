use directories_next::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Could not find project directories")]
    NoProjectDirs,
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON Error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DictatorConfig {
    pub api_url: String,
    pub api_key: String,
    pub default_model: String,
    pub theme: String,
}

impl Default for DictatorConfig {
    fn default() -> Self {
        DictatorConfig {
            api_url: "http://localhost:9934".to_string(), // Or your preferred default
            api_key: "".to_string(),
            default_model: "".to_string(),
            theme: "catppuccinMocha".to_string(), // Default theme
        }
    }
}

fn get_config_path() -> Result<PathBuf, ConfigError> {
    let proj_dirs = ProjectDirs::from("com", "YourCompany", "Dictator") // Adjust qualifier, org, app
        .ok_or(ConfigError::NoProjectDirs)?;
    let config_dir = proj_dirs.config_dir();
    fs::create_dir_all(config_dir)?;
    Ok(config_dir.join("config.json"))
}

pub fn load_config() -> Result<DictatorConfig, ConfigError> {
    let config_path = get_config_path()?;
    if !config_path.exists() {
        log::info!("Config file not found, creating default config.");
        let default_config = DictatorConfig::default();
        save_config(&default_config)?; // Save the default if it doesn't exist
        return Ok(default_config);
    }

    let mut file = fs::File::open(config_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let config: DictatorConfig = serde_json::from_str(&contents)?;
    Ok(config)
}

pub fn save_config(config: &DictatorConfig) -> Result<(), ConfigError> {
    let config_path = get_config_path()?;
    let json_string = serde_json::to_string_pretty(config)?;
    let mut file = fs::File::create(config_path)?;
    file.write_all(json_string.as_bytes())?;
    Ok(())
}
