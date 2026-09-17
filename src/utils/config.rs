use std::collections::BTreeMap;
use std::sync::{LazyLock, OnceLock};

use anyhow::{Result, anyhow, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::paths::CONFIG_DIR;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub enable_osd: bool,
    pub notifications: NotificationMode,
    pub api: ApiConfig,
    pub audio: AudioConfig,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationMode {
    All,
    ErrorsOnly,
    Off,
}

impl std::fmt::Display for NotificationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NotificationMode::All => "all",
            NotificationMode::ErrorsOnly => "errors_only",
            NotificationMode::Off => "off",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Provider {
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiConfig {
    pub active_provider: String,
    pub timeout: i64,
    pub providers: BTreeMap<String, Provider>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioConfig {
    pub sample_rate: i64,
    pub channels: i64,
    pub bit_depth: i64,
    pub frames_per_block: i64,
    pub max_duration_min: i64,
}

static ENV_KEY_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{env:([A-Za-z_][A-Za-z0-9_]*)\}").expect("valid regex"));

pub fn default_config() -> Config {
    let mut providers = BTreeMap::new();
    providers.insert(
        "openai".to_string(),
        Provider {
            endpoint: "https://api.openai.com/v1/audio/transcriptions".to_string(),
            key: String::new(),
            model: "gpt-4o-transcribe".to_string(),
        },
    );
    Config {
        enable_osd: true,
        notifications: NotificationMode::ErrorsOnly,
        api: ApiConfig {
            active_provider: "openai".to_string(),
            timeout: 60,
            providers,
        },
        audio: AudioConfig {
            sample_rate: 16000,
            channels: 1,
            bit_depth: 16,
            frames_per_block: 1024,
            max_duration_min: 5,
        },
    }
}

pub fn validate(config: &Config) -> Result<()> {
    if config.api.active_provider.is_empty() {
        bail!("active provider is required");
    }

    let Some(active) = config.api.providers.get(&config.api.active_provider) else {
        bail!(
            "active provider '{}' not found in providers",
            config.api.active_provider
        );
    };

    if active.endpoint.is_empty() {
        bail!(
            "endpoint is required for active provider '{}'",
            config.api.active_provider
        );
    }
    if active.key.is_empty() {
        bail!(
            "API key is required for active provider '{}'",
            config.api.active_provider
        );
    }
    if config.api.timeout <= 0 {
        bail!("API timeout must be > 0");
    }

    if config.audio.sample_rate <= 0 {
        bail!("audio sample rate must be positive");
    }
    if config.audio.channels <= 0 {
        bail!("audio channels must be positive");
    }
    if config.audio.bit_depth <= 0 {
        bail!("audio bit depth must be positive");
    }
    if config.audio.frames_per_block <= 0 {
        bail!("audio frames per block must be positive");
    }
    if config.audio.max_duration_min <= 0 {
        bail!("audio max duration min must be positive");
    }

    Ok(())
}

/// Expands `${env:VAR}` references. Returns the expanded string and the list of
/// variables that were referenced but not set (left verbatim in the output).
pub fn expand_env_substitutions(value: &str) -> (String, Vec<String>) {
    if !value.contains("${env:") {
        return (value.to_string(), Vec::new());
    }

    let mut out = String::with_capacity(value.len());
    let mut missing = Vec::new();
    let mut last = 0;

    for caps in ENV_KEY_PATTERN.captures_iter(value) {
        let whole = caps.get(0).expect("match");
        let name = &caps[1];
        out.push_str(&value[last..whole.start()]);
        match std::env::var(name) {
            Ok(env_value) => out.push_str(&env_value),
            Err(_) => {
                missing.push(name.to_string());
                out.push_str(whole.as_str());
            }
        }
        last = whole.end();
    }
    out.push_str(&value[last..]);

    (out, missing)
}

fn resolve_provider_keys(config: &mut Config) -> Result<()> {
    let active = config.api.active_provider.clone();
    let mut missing_for_active: Vec<String> = Vec::new();

    for (name, provider) in config.api.providers.iter_mut() {
        let (expanded, missing) = expand_env_substitutions(&provider.key);
        provider.key = expanded;
        if *name == active {
            missing_for_active.extend(missing);
        }
    }

    if !missing_for_active.is_empty() {
        let mut ordered: Vec<String> = Vec::new();
        for name in missing_for_active {
            if !ordered.contains(&name) {
                ordered.push(name);
            }
        }
        bail!(
            "missing env vars for active provider key: {}",
            ordered.join(", ")
        );
    }

    Ok(())
}

/// Recursively merges `overlay` into `base`: objects are merged key by key,
/// any other value replaces the base value.
fn deep_merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, value) in overlay_map {
                match base_map.get_mut(&key) {
                    Some(existing) => deep_merge(existing, value),
                    None => {
                        base_map.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

/// Applies `DICTATOR_<KEY>` environment overrides, where `<KEY>` is the
/// upper-cased dotted path of a leaf value (e.g. `DICTATOR_NOTIFICATIONS`,
/// `DICTATOR_API.TIMEOUT`). Values are coerced to the type of the existing leaf.
fn apply_env_overrides(value: &mut Value, prefix: &str) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                apply_env_overrides(child, &path);
            }
        }
        leaf => {
            if prefix.is_empty() {
                return;
            }
            let env_name = format!("DICTATOR_{}", prefix.to_uppercase());
            let Ok(raw) = std::env::var(&env_name) else {
                return;
            };
            let coerced = match leaf {
                Value::Bool(_) => raw.parse::<bool>().ok().map(Value::Bool),
                Value::Number(_) => raw
                    .parse::<i64>()
                    .ok()
                    .map(Value::from)
                    .or_else(|| raw.parse::<f64>().ok().map(Value::from)),
                _ => Some(Value::String(raw)),
            };
            if let Some(new_value) = coerced {
                *leaf = new_value;
            }
        }
    }
}

fn load_config() -> Result<Config> {
    let mut merged = serde_json::to_value(default_config()).expect("default config serializes");

    let config_path = CONFIG_DIR.join("config.json");
    match std::fs::read(&config_path) {
        Ok(bytes) => {
            let file_value: Value = serde_json::from_slice(&bytes)
                .map_err(|e| anyhow!("config: failed to parse: {e}"))?;
            deep_merge(&mut merged, file_value);
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => bail!("config: {err}"),
    }

    apply_env_overrides(&mut merged, "");

    let mut config: Config =
        serde_json::from_value(merged).map_err(|e| anyhow!("config: failed to parse: {e}"))?;

    resolve_provider_keys(&mut config).map_err(|e| anyhow!("config: {e}"))?;
    validate(&config).map_err(|e| anyhow!("config: failed to validate: {e}"))?;

    Ok(config)
}

static GLOBAL_CONFIG: OnceLock<Result<Config, String>> = OnceLock::new();

/// Loads the configuration once (defaults, then `config.json`, then
/// `DICTATOR_*` env overrides), resolving `${env:VAR}` provider keys and
/// validating the result.
pub fn get_config() -> Result<Config> {
    let result = GLOBAL_CONFIG.get_or_init(|| load_config().map_err(|e| format!("{e:#}")));
    match result {
        Ok(config) => Ok(config.clone()),
        Err(msg) => Err(anyhow!("{msg}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_env_and_reports_missing() {
        // SAFETY: test-only, single-threaded access to this variable name.
        unsafe { std::env::set_var("DICTATOR_TEST_KEY", "secret") };
        let (value, missing) =
            expand_env_substitutions("${env:DICTATOR_TEST_KEY}-${env:DICTATOR_TEST_MISSING}");
        assert_eq!(value, "secret-${env:DICTATOR_TEST_MISSING}");
        assert_eq!(missing, vec!["DICTATOR_TEST_MISSING".to_string()]);

        let (plain, missing) = expand_env_substitutions("plain");
        assert_eq!(plain, "plain");
        assert!(missing.is_empty());
    }

    #[test]
    fn validate_rejects_missing_key() {
        let cfg = default_config();
        let err = validate(&cfg).unwrap_err().to_string();
        assert_eq!(err, "API key is required for active provider 'openai'");
    }

    #[test]
    fn deep_merge_keeps_defaults() {
        let mut base = serde_json::to_value(default_config()).unwrap();
        deep_merge(
            &mut base,
            serde_json::json!({"audio": {"max_duration_min": 20}, "notifications": "off"}),
        );
        let cfg: Config = serde_json::from_value(base).unwrap();
        assert_eq!(cfg.audio.max_duration_min, 20);
        assert_eq!(cfg.audio.sample_rate, 16000);
        assert_eq!(cfg.notifications, NotificationMode::Off);
        assert_eq!(cfg.api.providers["openai"].model, "gpt-4o-transcribe");
    }
}
