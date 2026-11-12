//! Runtime choice of transcription provider.
//!
//! `config.json` defines the providers and names the default in
//! `api.active_provider`. Home Manager usually owns that file, so a switch
//! made while the daemon runs is kept in a separate writable file:
//!
//! ```json
//! {"version": 1, "active_provider": "openai"}
//! ```
//!
//! A missing file or key means "use the config default". Choosing the default
//! again removes the key, so a later change to the default takes effect.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

use crate::utils::{
    ApiConfig, CONFIG_DIR, Provider, create_private_dir, unresolved_env_references,
};

pub const RUNTIME_VERSION: u32 = 1;
const RUNTIME_FILE: &str = "runtime.json";

/// `$XDG_CONFIG_HOME/dictator/runtime.json`
pub fn runtime_path() -> PathBuf {
    CONFIG_DIR.join(RUNTIME_FILE)
}

#[derive(Debug, Serialize, Deserialize)]
struct RuntimeFile {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_provider: Option<String>,
    #[serde(flatten)]
    other: BTreeMap<String, Value>,
}

/// One configured provider as the daemon reports it. Never carries the key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSummary {
    pub name: String,
    pub model: String,
    pub endpoint: String,
    pub usable: bool,
    /// Why the provider cannot be selected; empty when usable.
    #[serde(default)]
    pub problem: String,
    pub active: bool,
    pub is_default: bool,
    pub send_vocabulary: bool,
}

/// Why `provider` cannot transcribe, or `None` when it can.
pub fn provider_problem(provider: &Provider) -> Option<String> {
    if provider.endpoint.trim().is_empty() {
        return Some("no endpoint is configured".to_string());
    }
    match unresolved_env_references(&provider.key).as_slice() {
        [] => {}
        [name] => return Some(format!("{name} is not set in the daemon's environment")),
        names => {
            return Some(format!(
                "{} are not set in the daemon's environment",
                names.join(", ")
            ));
        }
    }
    if provider.key.trim().is_empty() {
        return Some("no API key is configured".to_string());
    }
    None
}

/// Lists every configured provider in name order.
pub fn summaries(api: &ApiConfig, active: &str) -> Vec<ProviderSummary> {
    api.providers
        .iter()
        .map(|(name, provider)| {
            let problem = provider_problem(provider);
            ProviderSummary {
                name: name.clone(),
                model: provider.model.clone(),
                endpoint: provider.endpoint.clone(),
                usable: problem.is_none(),
                problem: problem.unwrap_or_default(),
                active: name == active,
                is_default: *name == api.active_provider,
                send_vocabulary: provider.send_vocabulary,
            }
        })
        .collect()
}

/// Picks the provider to use from the saved override. An override naming an
/// unknown or unusable provider falls back to the config default; the second
/// value then explains why.
pub fn resolve_active(api: &ApiConfig, requested: Option<&str>) -> (String, Option<String>) {
    let default = api.active_provider.clone();
    let Some(name) = requested.filter(|name| *name != default) else {
        return (default, None);
    };
    match api.providers.get(name) {
        None => {
            let warning = format!(
                "{RUNTIME_FILE} selects provider '{name}', which is not configured; using the default '{default}'"
            );
            (default, Some(warning))
        }
        Some(provider) => match provider_problem(provider) {
            Some(problem) => {
                let warning = format!(
                    "{RUNTIME_FILE} selects provider '{name}', which cannot be used ({problem}); using the default '{default}'"
                );
                (default, Some(warning))
            }
            None => (name.to_string(), None),
        },
    }
}

/// Reads the saved override. `None` means no file or no `active_provider`.
pub fn load_override(path: &Path) -> Result<Option<String>> {
    Ok(read_runtime(path)?.and_then(|file| file.active_provider))
}

fn read_runtime(path: &Path) -> Result<Option<RuntimeFile>> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let file: RuntimeFile = serde_json::from_slice(&data)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if file.version != RUNTIME_VERSION {
        bail!(
            "unsupported runtime settings version {} in {}",
            file.version,
            path.display()
        );
    }
    Ok(Some(file))
}

/// Saves `provider` as the override, or removes the override for `None`.
/// The file is deleted once it holds nothing else.
///
/// A file that is not valid JSON is moved aside (`runtime.json.corrupt`) and
/// replaced, so a damaged file cannot block switching for good. Read errors
/// and an unsupported version still fail, leaving the file alone.
pub fn save_override(path: &Path, provider: Option<&str>) -> Result<()> {
    let existing = match read_runtime(path) {
        Err(error) if error.downcast_ref::<serde_json::Error>().is_some() => {
            let aside = path.with_extension("json.corrupt");
            fs::rename(path, &aside)
                .with_context(|| format!("failed to move aside malformed {}", path.display()))?;
            warn!(path = %aside.display(), "moved aside malformed runtime settings");
            None
        }
        other => other?,
    };
    let mut file = existing.unwrap_or(RuntimeFile {
        version: RUNTIME_VERSION,
        active_provider: None,
        other: BTreeMap::new(),
    });
    file.active_provider = provider.map(str::to_string);
    if file.active_provider.is_none() && file.other.is_empty() {
        return match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err).with_context(|| format!("failed to remove {}", path.display())),
        };
    }
    write_runtime(path, &file)
}

fn write_runtime(path: &Path, file: &RuntimeFile) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("runtime settings path has no parent: {}", path.display()))?;
    create_private_dir(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let temp_path = parent.join(format!(
        ".{RUNTIME_FILE}.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> Result<()> {
        let mut temp = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp_path)
            .with_context(|| format!("failed to create {}", temp_path.display()))?;
        serde_json::to_writer_pretty(&mut temp, file)
            .context("failed to serialize runtime settings")?;
        temp.write_all(b"\n")
            .and_then(|()| temp.sync_all())
            .context("failed to write runtime settings")?;
        fs::rename(&temp_path, path)
            .with_context(|| format!("failed to replace {}", path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to restrict {}", path.display()))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("failed to sync {}", parent.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

/// The provider a process outside the daemon (the `retry` command) should
/// use: the saved override when usable, otherwise the config default.
pub fn effective_provider(api: &ApiConfig) -> String {
    let requested = load_override(&runtime_path()).unwrap_or_else(|err| {
        warn!(err = %format!("{err:#}"), "ignoring runtime provider override");
        None
    });
    let (active, warning) = resolve_active(api, requested.as_deref());
    if let Some(warning) = warning {
        warn!("{warning}");
    }
    active
}

/// The daemon's current provider, switched by IPC and kept in `runtime.json`.
pub struct ProviderSelector {
    api: ApiConfig,
    path: PathBuf,
    active: Mutex<String>,
}

impl ProviderSelector {
    /// Starts from the saved override. One that is unreadable, unknown or
    /// unusable is logged and the config default is used, so a stale
    /// override never stops the daemon from starting.
    pub fn load(api: &ApiConfig, path: PathBuf) -> Self {
        let requested = load_override(&path).unwrap_or_else(|err| {
            warn!(err = %format!("{err:#}"), "ignoring runtime provider override");
            None
        });
        let (active, warning) = resolve_active(api, requested.as_deref());
        if let Some(warning) = warning {
            warn!("{warning}");
        }
        Self {
            api: api.clone(),
            path,
            active: Mutex::new(active),
        }
    }

    pub fn active(&self) -> String {
        self.active.lock().unwrap().clone()
    }

    /// The model recorded for attempts made with `provider`.
    pub fn model(&self, provider: &str) -> Option<&str> {
        self.api
            .providers
            .get(provider)
            .map(|provider| provider.model.as_str())
    }

    pub fn list(&self) -> Vec<ProviderSummary> {
        summaries(&self.api, &self.active())
    }

    /// Makes `name` the active provider for the next transcription and saves
    /// the choice. Returns the updated list.
    pub fn select(&self, name: &str) -> Result<Vec<ProviderSummary>> {
        let name = name.trim();
        let Some(provider) = self.api.providers.get(name) else {
            let known: Vec<&str> = self.api.providers.keys().map(String::as_str).collect();
            bail!(
                "unknown provider '{name}'; configured providers: {}",
                known.join(", ")
            );
        };
        if let Some(problem) = provider_problem(provider) {
            bail!("provider '{name}' cannot be used: {problem}");
        }
        let mut active = self.active.lock().unwrap();
        let saved = (name != self.api.active_provider).then_some(name);
        save_override(&self.path, saved)?;
        if *active != name {
            info!(from = %*active, to = name, "transcription provider changed");
        }
        *active = name.to_string();
        drop(active);
        Ok(self.list())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(endpoint: &str, key: &str) -> Provider {
        Provider {
            endpoint: endpoint.to_string(),
            key: key.to_string(),
            model: "model".to_string(),
            send_vocabulary: false,
        }
    }

    fn api() -> ApiConfig {
        let mut providers = BTreeMap::new();
        providers.insert("siren".to_string(), {
            let mut siren = provider("http://siren:8000", "siren-key");
            siren.model = "large-v3".to_string();
            siren.send_vocabulary = true;
            siren
        });
        providers.insert(
            "openai".to_string(),
            provider("https://api.openai.com/v1", "sk-test"),
        );
        providers.insert(
            "groq".to_string(),
            provider(
                "https://api.groq.com/openai/v1",
                "${env:DICTATOR_TEST_UNSET_GROQ}",
            ),
        );
        ApiConfig {
            active_provider: "siren".to_string(),
            timeout: 60,
            providers,
        }
    }

    #[test]
    fn provider_problem_explains_each_unusable_provider() {
        let cases: &[(&str, Provider, Option<&str>)] = &[
            ("usable", provider("http://x", "k"), None),
            (
                "no endpoint",
                provider("  ", "k"),
                Some("no endpoint is configured"),
            ),
            (
                "no key",
                provider("http://x", ""),
                Some("no API key is configured"),
            ),
            (
                "unset variable",
                provider("http://x", "${env:OPENAI_API_KEY}"),
                Some("OPENAI_API_KEY is not set in the daemon's environment"),
            ),
            (
                "two unset variables",
                provider("http://x", "${env:A_KEY}-${env:B_KEY}-${env:A_KEY}"),
                Some("A_KEY, B_KEY are not set in the daemon's environment"),
            ),
        ];
        for (name, provider, expected) in cases {
            assert_eq!(provider_problem(provider).as_deref(), *expected, "{name}");
        }
    }

    #[test]
    fn summaries_mark_active_default_and_hide_keys() {
        let list = summaries(&api(), "openai");
        let names: Vec<_> = list.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["groq", "openai", "siren"]);

        let groq = &list[0];
        assert!(!groq.usable);
        assert_eq!(
            groq.problem,
            "DICTATOR_TEST_UNSET_GROQ is not set in the daemon's environment"
        );
        assert!(list[1].active && !list[1].is_default && list[1].usable);
        assert!(list[1].problem.is_empty());
        assert!(!list[2].active && list[2].is_default && list[2].send_vocabulary);
        assert_eq!(list[2].model, "large-v3");

        let encoded = serde_json::to_string(&list).unwrap();
        assert!(!encoded.contains("sk-test") && !encoded.contains("siren-key"));
    }

    #[test]
    fn resolve_active_falls_back_to_the_default() {
        let api = api();
        let cases: &[(Option<&str>, &str, bool)] = &[
            (None, "siren", false),
            (Some("siren"), "siren", false),
            (Some("openai"), "openai", false),
            (Some("missing"), "siren", true),
            (Some("groq"), "siren", true),
        ];
        for (requested, expected, warns) in cases {
            let (active, warning) = resolve_active(&api, *requested);
            assert_eq!(active, *expected, "{requested:?}");
            assert_eq!(warning.is_some(), *warns, "{requested:?}: {warning:?}");
        }
        let (_, warning) = resolve_active(&api, Some("groq"));
        assert!(
            warning
                .unwrap()
                .contains("DICTATOR_TEST_UNSET_GROQ is not set"),
            "the warning names the reason"
        );
    }

    #[test]
    fn selector_loads_the_override_or_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RUNTIME_FILE);
        let cases: &[(&str, Option<&[u8]>, &str)] = &[
            ("no file", None, "siren"),
            ("no key", Some(br#"{"version": 1}"#), "siren"),
            (
                "usable override",
                Some(br#"{"version": 1, "active_provider": "openai"}"#),
                "openai",
            ),
            (
                "unknown override",
                Some(br#"{"version": 1, "active_provider": "gone"}"#),
                "siren",
            ),
            (
                "unusable override",
                Some(br#"{"version": 1, "active_provider": "groq"}"#),
                "siren",
            ),
            (
                "future version",
                Some(br#"{"version": 2, "active_provider": "openai"}"#),
                "siren",
            ),
            ("corrupt", Some(b"{"), "siren"),
        ];
        for (name, contents, expected) in cases {
            match contents {
                Some(contents) => fs::write(&path, contents).unwrap(),
                None => {
                    let _ = fs::remove_file(&path);
                }
            }
            let selector = ProviderSelector::load(&api(), path.clone());
            assert_eq!(selector.active(), *expected, "{name}");
        }
    }

    #[test]
    fn a_malformed_file_is_moved_aside_but_a_newer_one_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RUNTIME_FILE);

        fs::write(&path, b"{").unwrap();
        save_override(&path, Some("openai")).unwrap();
        assert_eq!(load_override(&path).unwrap().as_deref(), Some("openai"));
        assert_eq!(fs::read(path.with_extension("json.corrupt")).unwrap(), b"{");

        let newer = br#"{"version": 2, "active_provider": "siren"}"#;
        fs::write(&path, newer).unwrap();
        assert!(save_override(&path, Some("openai")).is_err());
        assert_eq!(fs::read(&path).unwrap(), newer);
    }

    #[test]
    fn selection_persists_privately_and_default_removes_the_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dictator").join(RUNTIME_FILE);
        let selector = ProviderSelector::load(&api(), path.clone());
        assert_eq!(selector.active(), "siren");

        let list = selector.select(" openai ").unwrap();
        assert_eq!(selector.active(), "openai");
        assert!(
            list.iter()
                .any(|entry| entry.name == "openai" && entry.active)
        );
        let stored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            stored,
            serde_json::json!({"version": 1, "active_provider": "openai"})
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            ProviderSelector::load(&api(), path.clone()).active(),
            "openai",
            "a restarted daemon keeps the choice"
        );

        selector.select("siren").unwrap();
        assert_eq!(selector.active(), "siren");
        assert!(!path.exists(), "choosing the default removes the override");

        let mut changed_default = api();
        changed_default.active_provider = "openai".to_string();
        assert_eq!(
            ProviderSelector::load(&changed_default, path.clone()).active(),
            "openai",
            "a new config default takes effect"
        );

        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn selection_rejects_unknown_and_unusable_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RUNTIME_FILE);
        let selector = ProviderSelector::load(&api(), path.clone());

        let unknown = selector.select("nope").unwrap_err().to_string();
        assert_eq!(
            unknown,
            "unknown provider 'nope'; configured providers: groq, openai, siren"
        );
        let unusable = selector.select("groq").unwrap_err().to_string();
        assert_eq!(
            unusable,
            "provider 'groq' cannot be used: DICTATOR_TEST_UNSET_GROQ is not set in the daemon's environment"
        );
        assert_eq!(selector.active(), "siren");
        assert!(!path.exists());
    }

    #[test]
    fn removing_the_override_keeps_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RUNTIME_FILE);
        fs::write(
            &path,
            br#"{"version": 1, "active_provider": "openai", "later": true}"#,
        )
        .unwrap();

        save_override(&path, None).unwrap();

        let stored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored, serde_json::json!({"version": 1, "later": true}));
        assert_eq!(load_override(&path).unwrap(), None);
    }
}
