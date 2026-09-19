use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::utils::CONFIG_DIR;

const PREFERENCES_VERSION: u32 = 1;
const PREFERENCES_FILE: &str = "preferences.json";
static PREFERENCES_LOCK: Mutex<()> = Mutex::new(());

/// One row in the user-controlled microphone priority list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Microphone {
    pub id: String,
    pub name: String,
    pub connected: bool,
    pub is_default: bool,
}

/// The complete ordered microphone preference list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MicrophonePreferences {
    pub microphones: Vec<Microphone>,
}

/// Reads the saved order, discovers current inputs, and appends newly seen
/// microphones. A missing file is seeded once from current discovery.
pub fn microphone_preferences() -> Result<MicrophonePreferences> {
    let _guard = PREFERENCES_LOCK.lock().unwrap();
    DeviceManager::system().preferences()
}

/// Replaces the microphone priority order.
///
/// `ordered_ids` must contain every saved microphone exactly once. This makes
/// stale UI updates fail instead of dropping a disconnected or newly found
/// device from the preference file.
pub fn reorder_microphones(ordered_ids: &[String]) -> Result<MicrophonePreferences> {
    let _guard = PREFERENCES_LOCK.lock().unwrap();
    DeviceManager::system().reorder(ordered_ids)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedMicrophone {
    pub id: String,
    pub name: String,
    pub source_name: String,
}

/// Resolves capture once. The returned Pulse source name remains fixed for the
/// lifetime of the recording, even if preferences or defaults change.
pub(crate) fn resolve_microphone() -> Result<ResolvedMicrophone> {
    let _guard = PREFERENCES_LOCK.lock().unwrap();
    DeviceManager::system().resolve()
}

struct DeviceManager {
    preferences_path: PathBuf,
}

impl DeviceManager {
    fn system() -> Self {
        Self {
            preferences_path: CONFIG_DIR.join(PREFERENCES_FILE),
        }
    }

    #[cfg(test)]
    fn at(preferences_path: PathBuf) -> Self {
        Self { preferences_path }
    }

    fn preferences(&self) -> Result<MicrophonePreferences> {
        let _file_lock = lock_preferences(&self.preferences_path)?;
        let discovery = discover_microphones()?;
        let (saved, changed) = self.load_and_reconcile(&discovery)?;
        if changed {
            write_preferences(&self.preferences_path, &saved)?;
        }
        Ok(view_preferences(&saved, &discovery))
    }

    fn reorder(&self, ordered_ids: &[String]) -> Result<MicrophonePreferences> {
        let _file_lock = lock_preferences(&self.preferences_path)?;
        let discovery = discover_microphones()?;
        let (mut saved, reconciled) = self.load_and_reconcile(&discovery)?;
        validate_reorder(&saved.microphones, ordered_ids)?;
        let order_changed = saved
            .microphones
            .iter()
            .map(|microphone| &microphone.id)
            .ne(ordered_ids.iter());

        let mut by_id: HashMap<_, _> = saved
            .microphones
            .drain(..)
            .map(|microphone| (microphone.id.clone(), microphone))
            .collect();
        saved.microphones = ordered_ids
            .iter()
            .map(|id| by_id.remove(id).expect("validated microphone id"))
            .collect();
        if reconciled || order_changed {
            write_preferences(&self.preferences_path, &saved)?;
        }
        Ok(view_preferences(&saved, &discovery))
    }

    fn resolve(&self) -> Result<ResolvedMicrophone> {
        let _file_lock = lock_preferences(&self.preferences_path)?;
        let discovery = discover_microphones()?;
        let (saved, changed) = self.load_and_reconcile(&discovery)?;
        if changed {
            write_preferences(&self.preferences_path, &saved)?;
        }

        let selected = select_microphone(&saved, &discovery)
            .ok_or_else(|| anyhow!("failed to open audio stream: no connected input device"))?;

        Ok(ResolvedMicrophone {
            id: selected.id.clone(),
            name: selected.name.clone(),
            source_name: selected.source_name.clone(),
        })
    }

    fn load_and_reconcile(&self, discovery: &Discovery) -> Result<(SavedPreferences, bool)> {
        let loaded = read_preferences(&self.preferences_path)?;
        let missing = loaded.is_none();
        let mut saved = loaded.unwrap_or_default();
        let changed = reconcile(&mut saved, discovery);
        Ok((saved, missing || changed))
    }
}

fn select_microphone<'a>(
    saved: &SavedPreferences,
    discovery: &'a Discovery,
) -> Option<&'a DiscoveredMicrophone> {
    let connected: HashMap<_, _> = discovery
        .microphones
        .iter()
        .map(|microphone| (microphone.id.as_str(), microphone))
        .collect();
    saved
        .microphones
        .iter()
        .find_map(|saved| connected.get(saved.id.as_str()).copied())
        .or_else(|| {
            discovery
                .default_source
                .as_deref()
                .and_then(|name| discovery.microphones.iter().find(|m| m.source_name == name))
        })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SavedPreferences {
    version: u32,
    #[serde(default)]
    microphones: Vec<SavedMicrophone>,
    #[serde(flatten)]
    other: BTreeMap<String, Value>,
}

impl Default for SavedPreferences {
    fn default() -> Self {
        Self {
            version: PREFERENCES_VERSION,
            microphones: Vec::new(),
            other: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SavedMicrophone {
    id: String,
    name: String,
}

#[derive(Clone, Debug)]
struct DiscoveredMicrophone {
    id: String,
    name: String,
    source_name: String,
}

#[derive(Clone, Debug, Default)]
struct Discovery {
    microphones: Vec<DiscoveredMicrophone>,
    default_source: Option<String>,
}

fn discover_microphones() -> Result<Discovery> {
    let sources = Command::new("pactl")
        .args(["--format=json", "list", "sources"])
        .output()
        .context("failed to run pactl for microphone discovery")?;
    if !sources.status.success() {
        bail!(
            "failed to discover microphones with pactl: {}",
            String::from_utf8_lossy(&sources.stderr).trim()
        );
    }
    let default = Command::new("pactl")
        .arg("get-default-source")
        .output()
        .context("failed to query the default microphone with pactl")?;
    if !default.status.success() {
        bail!(
            "failed to query the default microphone with pactl: {}",
            String::from_utf8_lossy(&default.stderr).trim()
        );
    }
    let default_source = String::from_utf8(default.stdout)
        .context("pactl returned a non-UTF-8 default source name")?
        .trim()
        .to_owned();

    discovery_from_pactl_json(
        &sources.stdout,
        (!default_source.is_empty()).then_some(default_source),
    )
}

fn discovery_from_pactl_json(data: &[u8], default_source: Option<String>) -> Result<Discovery> {
    let values: Vec<Value> =
        serde_json::from_slice(data).context("failed to parse pactl microphone data")?;
    let mut by_id: HashMap<String, DiscoveredMicrophone> = HashMap::new();
    let mut order = Vec::new();

    for source in values {
        let Some(source_name) = source.get("name").and_then(Value::as_str) else {
            continue;
        };
        let is_monitor = source
            .get("monitor_source")
            .is_some_and(|monitor| !monitor.as_str().unwrap_or_default().is_empty());
        if is_monitor {
            continue;
        }
        let properties = source.get("properties").and_then(Value::as_object);
        if properties
            .and_then(|p| p.get("device.class"))
            .and_then(Value::as_str)
            == Some("monitor")
        {
            continue;
        }

        let id = stable_device_id(properties, source_name);
        let name = source
            .get("description")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .or_else(|| {
                properties
                    .and_then(|p| p.get("device.description"))
                    .and_then(Value::as_str)
            })
            .unwrap_or(source_name)
            .to_owned();
        let microphone = DiscoveredMicrophone {
            id: id.clone(),
            name,
            source_name: source_name.to_owned(),
        };
        match by_id.get_mut(&id) {
            Some(existing) if default_source.as_deref() == Some(source_name) => {
                *existing = microphone;
            }
            Some(_) => {}
            None => {
                order.push(id.clone());
                by_id.insert(id, microphone);
            }
        }
    }

    order.sort_by(|left, right| {
        let left_default = default_source.as_deref() == Some(by_id[left].source_name.as_str());
        let right_default = default_source.as_deref() == Some(by_id[right].source_name.as_str());
        right_default.cmp(&left_default).then_with(|| {
            by_id[left]
                .name
                .to_lowercase()
                .cmp(&by_id[right].name.to_lowercase())
        })
    });

    Ok(Discovery {
        microphones: order
            .into_iter()
            .map(|id| by_id.remove(&id).expect("known microphone id"))
            .collect(),
        default_source,
    })
}

fn stable_device_id(
    properties: Option<&serde_json::Map<String, Value>>,
    source_name: &str,
) -> String {
    let property = |key: &str| {
        properties
            .and_then(|values| values.get(key))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    if let Some(address) = property("api.bluez5.address").or_else(|| property("device.string"))
        && (property("device.bus") == Some("bluetooth") || source_name.starts_with("bluez_"))
    {
        return format!("bluetooth:{address}");
    }
    if let Some(path) = property("device.bus_path") {
        return format!("bus:{path}");
    }
    if let Some(device) = property("device.name") {
        return format!("device:{device}");
    }
    format!("source:{source_name}")
}

fn reconcile(saved: &mut SavedPreferences, discovery: &Discovery) -> bool {
    let discovered: HashMap<_, _> = discovery
        .microphones
        .iter()
        .map(|microphone| (microphone.id.as_str(), microphone))
        .collect();
    let mut changed = false;
    for microphone in &mut saved.microphones {
        if let Some(current) = discovered.get(microphone.id.as_str())
            && microphone.name != current.name
        {
            microphone.name.clone_from(&current.name);
            changed = true;
        }
    }

    let mut known: HashSet<_> = saved
        .microphones
        .iter()
        .map(|microphone| microphone.id.clone())
        .collect();
    for current in &discovery.microphones {
        if known.insert(current.id.clone()) {
            saved.microphones.push(SavedMicrophone {
                id: current.id.clone(),
                name: current.name.clone(),
            });
            changed = true;
        }
    }
    changed
}

fn view_preferences(saved: &SavedPreferences, discovery: &Discovery) -> MicrophonePreferences {
    let connected: HashMap<_, _> = discovery
        .microphones
        .iter()
        .map(|microphone| (microphone.id.as_str(), microphone))
        .collect();
    MicrophonePreferences {
        microphones: saved
            .microphones
            .iter()
            .map(|saved| {
                let current = connected.get(saved.id.as_str());
                Microphone {
                    id: saved.id.clone(),
                    name: current.map_or_else(|| saved.name.clone(), |m| m.name.clone()),
                    connected: current.is_some(),
                    is_default: current.is_some_and(|m| {
                        discovery.default_source.as_deref() == Some(m.source_name.as_str())
                    }),
                }
            })
            .collect(),
    }
}

fn validate_reorder(saved: &[SavedMicrophone], ordered_ids: &[String]) -> Result<()> {
    if saved.len() != ordered_ids.len() {
        bail!(
            "microphone priority update is stale: expected {} devices, got {}",
            saved.len(),
            ordered_ids.len()
        );
    }
    let expected: HashSet<_> = saved
        .iter()
        .map(|microphone| microphone.id.as_str())
        .collect();
    let actual: HashSet<_> = ordered_ids.iter().map(String::as_str).collect();
    if actual.len() != ordered_ids.len() || actual != expected {
        bail!("microphone priority must contain every saved device exactly once");
    }
    Ok(())
}

fn read_preferences(path: &Path) -> Result<Option<SavedPreferences>> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let preferences: SavedPreferences = serde_json::from_slice(&data)
        .with_context(|| format!("failed to parse {}; refusing to reset it", path.display()))?;
    if preferences.version != PREFERENCES_VERSION {
        bail!(
            "unsupported preferences version {} in {}; refusing to reset it",
            preferences.version,
            path.display()
        );
    }
    let mut ids = HashSet::new();
    if preferences
        .microphones
        .iter()
        .any(|microphone| microphone.id.is_empty() || !ids.insert(microphone.id.as_str()))
    {
        bail!(
            "invalid microphone priority in {}; refusing to reset it",
            path.display()
        );
    }
    Ok(Some(preferences))
}

struct PreferencesFileLock(File);

impl Drop for PreferencesFileLock {
    fn drop(&mut self) {
        // Closing the file also releases the lock if this best-effort unlock
        // fails. Drop cannot report an unlock error to the caller.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn lock_preferences(path: &Path) -> Result<PreferencesFileLock> {
    let parent = ensure_private_parent(path)?;
    let lock_path = parent.join(".preferences.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .with_context(|| format!("failed to open preferences lock {}", lock_path.display()))?;
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).with_context(|| {
        format!(
            "failed to restrict preferences lock {}",
            lock_path.display()
        )
    })?;
    loop {
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if result == 0 {
            return Ok(PreferencesFileLock(file));
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(err)
                .with_context(|| format!("failed to lock preferences {}", lock_path.display()));
        }
    }
}

fn ensure_private_parent(path: &Path) -> Result<&Path> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("preferences path has no parent: {}", path.display()))?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(parent).with_context(|| {
        format!(
            "failed to create preferences directory {}",
            parent.display()
        )
    })?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).with_context(|| {
        format!(
            "failed to restrict preferences directory {}",
            parent.display()
        )
    })?;
    Ok(parent)
}

fn write_preferences(path: &Path, preferences: &SavedPreferences) -> Result<()> {
    let parent = ensure_private_parent(path)?;

    let temp_path = parent.join(format!(
        ".{PREFERENCES_FILE}.{}.{}.tmp",
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
        serde_json::to_writer_pretty(&mut temp, preferences)
            .context("failed to serialize microphone preferences")?;
        temp.write_all(b"\n")
            .context("failed to finish microphone preferences")?;
        temp.sync_all()
            .context("failed to sync microphone preferences")?;
        fs::rename(&temp_path, path)
            .with_context(|| format!("failed to replace {}", path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to restrict {}", path.display()))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| {
                format!("failed to sync preferences directory {}", parent.display())
            })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCES: &str = r#"[
      {
        "name": "alsa_output.pci.monitor",
        "description": "Monitor of Speakers",
        "monitor_source": "alsa_output.pci",
        "properties": {"device.class": "monitor", "device.name": "alsa_card.pci"}
      },
      {
        "name": "bluez_input.11_22_33.headset-head-unit",
        "description": "Travel Headset",
        "monitor_source": "",
        "properties": {
          "device.bus": "bluetooth",
          "device.string": "11:22:33:44:55:66",
          "device.name": "bluez_card.11_22_33"
        }
      },
      {
        "name": "alsa_input.usb-mic.mono-fallback",
        "description": "Desk microphone",
        "monitor_source": "",
        "properties": {
          "device.bus_path": "pci-0000:00:14.0-usb-0:5:1.0",
          "device.name": "alsa_card.usb-mic"
        }
      }
    ]"#;

    fn discovery() -> Discovery {
        discovery_from_pactl_json(
            SOURCES.as_bytes(),
            Some("alsa_input.usb-mic.mono-fallback".into()),
        )
        .unwrap()
    }

    #[test]
    fn pactl_discovery_uses_stable_physical_ids_and_ignores_monitors() {
        let discovery = discovery();
        assert_eq!(discovery.microphones.len(), 2);
        assert_eq!(
            discovery.microphones[0].id,
            "bus:pci-0000:00:14.0-usb-0:5:1.0"
        );
        assert_eq!(discovery.microphones[1].id, "bluetooth:11:22:33:44:55:66");
    }

    #[test]
    fn bluetooth_profile_names_resolve_to_the_same_identity() {
        let properties =
            serde_json::from_value::<serde_json::Map<String, Value>>(serde_json::json!({
                "device.bus": "bluetooth",
                "device.string": "11:22:33:44:55:66"
            }))
            .unwrap();
        let first = stable_device_id(Some(&properties), "bluez_input.11_22_33.headset-head-unit");
        assert_eq!(first, "bluetooth:11:22:33:44:55:66");
    }

    #[test]
    fn reconciliation_retains_disconnected_rank_and_appends_new_devices() {
        let mut saved = SavedPreferences {
            microphones: vec![
                SavedMicrophone {
                    id: "bluetooth:11:22:33:44:55:66".into(),
                    name: "Old headset name".into(),
                },
                SavedMicrophone {
                    id: "device:disconnected".into(),
                    name: "Disconnected microphone".into(),
                },
            ],
            ..SavedPreferences::default()
        };
        assert!(reconcile(&mut saved, &discovery()));
        assert_eq!(
            saved
                .microphones
                .iter()
                .map(|microphone| microphone.id.as_str())
                .collect::<Vec<_>>(),
            [
                "bluetooth:11:22:33:44:55:66",
                "device:disconnected",
                "bus:pci-0000:00:14.0-usb-0:5:1.0",
            ]
        );
        let view = view_preferences(&saved, &discovery());
        assert!(view.microphones[0].connected);
        assert!(!view.microphones[1].connected);
        assert_eq!(view.microphones[0].name, "Travel Headset");
    }

    #[test]
    fn selection_uses_first_connected_priority_and_then_system_default() {
        let discovery = discovery();
        let saved = SavedPreferences {
            microphones: vec![
                SavedMicrophone {
                    id: "device:disconnected".into(),
                    name: "Disconnected microphone".into(),
                },
                SavedMicrophone {
                    id: "bluetooth:11:22:33:44:55:66".into(),
                    name: "Travel Headset".into(),
                },
                SavedMicrophone {
                    id: "bus:pci-0000:00:14.0-usb-0:5:1.0".into(),
                    name: "Desk microphone".into(),
                },
            ],
            ..SavedPreferences::default()
        };
        assert_eq!(
            select_microphone(&saved, &discovery).unwrap().source_name,
            "bluez_input.11_22_33.headset-head-unit"
        );

        let unsaved = SavedPreferences::default();
        assert_eq!(
            select_microphone(&unsaved, &discovery).unwrap().source_name,
            "alsa_input.usb-mic.mono-fallback"
        );
    }

    #[test]
    fn seed_write_is_private_atomic_and_preserves_unknown_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config").join(PREFERENCES_FILE);
        let manager = DeviceManager::at(path.clone());
        let mut saved = SavedPreferences::default();
        saved
            .other
            .insert("window".into(), serde_json::json!({"width": 900}));
        reconcile(&mut saved, &discovery());
        write_preferences(&path, &saved).unwrap();

        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let loaded = read_preferences(&manager.preferences_path)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.other["window"]["width"], 900);
        assert_eq!(loaded.microphones.len(), 2);
        assert!(fs::read_dir(path.parent().unwrap()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[test]
    fn corrupt_preferences_are_not_reset_or_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(PREFERENCES_FILE);
        fs::write(&path, b"{broken").unwrap();

        let error = read_preferences(&path).unwrap_err().to_string();

        assert!(error.contains("refusing to reset it"));
        assert_eq!(fs::read(&path).unwrap(), b"{broken");
    }

    #[test]
    fn reorder_requires_an_exact_permutation() {
        let saved = vec![
            SavedMicrophone {
                id: "one".into(),
                name: "One".into(),
            },
            SavedMicrophone {
                id: "two".into(),
                name: "Two".into(),
            },
        ];
        assert!(validate_reorder(&saved, &["two".into(), "one".into()]).is_ok());
        assert!(validate_reorder(&saved, &["one".into()]).is_err());
        assert!(validate_reorder(&saved, &["one".into(), "one".into()]).is_err());
    }

    #[test]
    fn preference_lock_serializes_independent_open_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config").join(PREFERENCES_FILE);
        let first = lock_preferences(&path).unwrap();
        let competing_path = path.clone();
        let (attempting_tx, attempting_rx) = std::sync::mpsc::channel();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let contender = std::thread::spawn(move || {
            attempting_tx.send(()).unwrap();
            let _second = lock_preferences(&competing_path).unwrap();
            acquired_tx.send(()).unwrap();
        });

        attempting_rx.recv().unwrap();
        assert!(
            acquired_rx
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
        drop(first);
        acquired_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        contender.join().unwrap();
        assert_eq!(
            fs::metadata(path.parent().unwrap().join(".preferences.lock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
