use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::utils::{CONFIG_DIR, bounded_output};

const PREFERENCES_VERSION: u32 = 1;
const USB_ID_PREFIX: &str = "usb:";
const PREFERENCES_FILE: &str = "preferences.json";
const PACTL_TIMEOUT: Duration = Duration::from_secs(2);
const PACTL_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
static PREFERENCES_LOCK: Mutex<()> = Mutex::new(());

/// One row in the user-controlled microphone priority list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Microphone {
    pub id: String,
    pub name: String,
    pub connected: bool,
    pub is_default: bool,
    /// Bluetooth inputs keep their place in the list but are never used.
    pub bluetooth: bool,
}

/// The complete ordered microphone preference list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MicrophonePreferences {
    pub microphones: Vec<Microphone>,
}

/// Reads the saved order, discovers current inputs, and appends newly seen
/// microphones. A missing file is seeded once from current discovery.
pub fn microphone_preferences() -> Result<MicrophonePreferences> {
    let discovery = discover_microphones()?;
    let _guard = PREFERENCES_LOCK.lock().unwrap();
    DeviceManager::system().preferences(&discovery)
}

/// Replaces the microphone priority order.
///
/// `ordered_ids` must contain every saved microphone exactly once. This makes
/// stale UI updates fail instead of dropping a disconnected or newly found
/// device from the preference file.
pub fn reorder_microphones(ordered_ids: &[String]) -> Result<MicrophonePreferences> {
    let discovery = discover_microphones()?;
    let _guard = PREFERENCES_LOCK.lock().unwrap();
    DeviceManager::system().reorder(ordered_ids, &discovery)
}

/// Removes a disconnected microphone from the priority list.
///
/// A connected microphone cannot be removed, because the next discovery
/// would add it back at the end of the list.
pub fn remove_microphone(id: &str) -> Result<MicrophonePreferences> {
    let discovery = discover_microphones()?;
    let _guard = PREFERENCES_LOCK.lock().unwrap();
    DeviceManager::system().remove(id, &discovery)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedMicrophone {
    pub id: String,
    pub name: String,
    pub source_name: String,
}

/// A resolved microphone and the preferences file version it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolution {
    pub microphone: ResolvedMicrophone,
    /// Taken under the preferences lock after any write the resolution made
    /// (seeding the file, appending a new device, moving saved USB entries to
    /// port-independent ids), so that write does not look like a later
    /// priority change. Each of those writes happens once: resolving again
    /// with the same devices leaves the file untouched.
    pub fingerprint: Option<Fingerprint>,
}

/// Resolves capture once. The returned Pulse source name remains fixed for the
/// lifetime of a capture stream, even if preferences or defaults change.
pub(crate) fn resolve_microphone() -> Result<Resolution> {
    let discovery = discover_microphones()?;
    let _guard = PREFERENCES_LOCK.lock().unwrap();
    DeviceManager::system().resolve(&discovery)
}

/// Identifies one version of the preferences file without reading it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    inode: u64,
    len: u64,
    modified_ns: i128,
}

/// Lets the start path notice a priority change (from the desktop app or
/// another process) with one `stat` instead of a pactl round trip.
pub(crate) fn preferences_fingerprint() -> Option<Fingerprint> {
    fingerprint_at(&CONFIG_DIR.join(PREFERENCES_FILE))
}

#[cfg(test)]
impl Fingerprint {
    pub(crate) fn for_test(version: u64) -> Self {
        Self {
            inode: version,
            len: 0,
            modified_ns: 0,
        }
    }
}

fn fingerprint_at(path: &Path) -> Option<Fingerprint> {
    let metadata = fs::metadata(path).ok()?;
    Some(Fingerprint {
        inode: metadata.ino(),
        len: metadata.len(),
        modified_ns: i128::from(metadata.mtime()) * 1_000_000_000
            + i128::from(metadata.mtime_nsec()),
    })
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

    fn preferences(&self, discovery: &Discovery) -> Result<MicrophonePreferences> {
        let _file_lock = lock_preferences(&self.preferences_path)?;
        let (saved, discovery, changed) = self.load_and_reconcile(discovery)?;
        let discovery = &discovery;
        if changed {
            write_preferences(&self.preferences_path, &saved)?;
        }
        Ok(view_preferences(&saved, discovery))
    }

    fn reorder(
        &self,
        ordered_ids: &[String],
        discovery: &Discovery,
    ) -> Result<MicrophonePreferences> {
        let _file_lock = lock_preferences(&self.preferences_path)?;
        let (mut saved, discovery, reconciled) = self.load_and_reconcile(discovery)?;
        let discovery = &discovery;
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
        Ok(view_preferences(&saved, discovery))
    }

    fn remove(&self, id: &str, discovery: &Discovery) -> Result<MicrophonePreferences> {
        let _file_lock = lock_preferences(&self.preferences_path)?;
        let (mut saved, discovery, _) = self.load_and_reconcile(discovery)?;
        let discovery = &discovery;
        if let Some(connected) = discovery
            .microphones
            .iter()
            .find(|microphone| microphone.id == id)
        {
            bail!(
                "\"{}\" is connected; disconnect it before removing it, or it is added back on the next discovery",
                connected.name
            );
        }
        let Some(index) = saved
            .microphones
            .iter()
            .position(|microphone| microphone.id == id)
        else {
            bail!("no saved microphone has the id {id:?}");
        };
        saved.microphones.remove(index);
        write_preferences(&self.preferences_path, &saved)?;
        Ok(view_preferences(&saved, discovery))
    }

    fn resolve(&self, discovery: &Discovery) -> Result<Resolution> {
        let _file_lock = lock_preferences(&self.preferences_path)?;
        let (saved, discovery, changed) = self.load_and_reconcile(discovery)?;
        let discovery = &discovery;
        if changed {
            write_preferences(&self.preferences_path, &saved)?;
        }

        let selected = select_microphone(&saved, discovery).ok_or_else(|| {
            if discovery.microphones.is_empty() {
                anyhow!("failed to open audio stream: no connected input device")
            } else {
                anyhow!("no non-Bluetooth microphone available")
            }
        })?;

        Ok(Resolution {
            microphone: ResolvedMicrophone {
                id: selected.id.clone(),
                name: selected.name.clone(),
                source_name: selected.source_name.clone(),
            },
            fingerprint: fingerprint_at(&self.preferences_path),
        })
    }

    /// Returns the saved list, the discovery aligned to the ids it was saved
    /// under, and whether the list needs writing.
    fn load_and_reconcile(
        &self,
        discovery: &Discovery,
    ) -> Result<(SavedPreferences, Discovery, bool)> {
        let loaded = read_preferences(&self.preferences_path)?;
        let missing = loaded.is_none();
        let mut saved = loaded.unwrap_or_default();
        let discovery = keep_saved_discriminators(discovery, &saved);
        let changed = reconcile(&mut saved, &discovery);
        Ok((saved, discovery, missing || changed))
    }
}

/// Picks the first connected microphone in priority order, then the system
/// default, then any connected input. Bluetooth inputs are never selected,
/// even when preferred or default.
fn select_microphone<'a>(
    saved: &SavedPreferences,
    discovery: &'a Discovery,
) -> Option<&'a DiscoveredMicrophone> {
    let usable: HashMap<_, _> = discovery
        .microphones
        .iter()
        .filter(|microphone| !microphone.bluetooth)
        .map(|microphone| (microphone.id.as_str(), microphone))
        .collect();
    saved
        .microphones
        .iter()
        .find_map(|saved| usable.get(saved.id.as_str()).copied())
        .or_else(|| {
            discovery.default_source.as_deref().and_then(|name| {
                usable
                    .values()
                    .copied()
                    .find(|microphone| microphone.source_name == name)
            })
        })
        .or_else(|| {
            discovery
                .microphones
                .iter()
                .find(|microphone| !microphone.bluetooth)
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
    bluetooth: bool,
    /// The port-dependent `bus:` id older versions saved for this USB device.
    legacy_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct Discovery {
    microphones: Vec<DiscoveredMicrophone>,
    default_source: Option<String>,
}

fn discover_microphones() -> Result<Discovery> {
    discover_microphones_with(|| Command::new("pactl"), PACTL_TIMEOUT)
}

fn discover_microphones_with(
    mut new_command: impl FnMut() -> Command,
    timeout: Duration,
) -> Result<Discovery> {
    let mut sources_command = new_command();
    let sources = bounded_output(
        sources_command.args(["--format=json", "list", "sources"]),
        timeout,
        PACTL_OUTPUT_LIMIT,
    )
    .context("failed to run pactl for microphone discovery")?;
    if sources.timed_out {
        bail!("microphone discovery timed out after {timeout:?}");
    }
    if sources.stdout_truncated {
        bail!("pactl microphone discovery output exceeded {PACTL_OUTPUT_LIMIT} bytes");
    }
    if !sources.status.success() {
        bail!(
            "failed to discover microphones with pactl: {}",
            String::from_utf8_lossy(&sources.stderr).trim()
        );
    }
    let mut default_command = new_command();
    let default = bounded_output(
        default_command.arg("get-default-source"),
        timeout,
        PACTL_OUTPUT_LIMIT,
    )
    .context("failed to query the default microphone with pactl")?;
    if default.timed_out {
        bail!("default microphone query timed out after {timeout:?}");
    }
    if default.stdout_truncated {
        bail!("pactl default microphone output exceeded {PACTL_OUTPUT_LIMIT} bytes");
    }
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
    let mut found: Vec<(DiscoveredMicrophone, String)> = Vec::new();

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
        let bluetooth = is_bluetooth(properties, source_name);
        let property = |key: &str| {
            properties
                .and_then(|values| values.get(key))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        };
        let legacy_id = property("device.bus_path")
            .filter(|_| id.starts_with(USB_ID_PREFIX))
            .map(|path| format!("bus:{path}"));
        let card = property("device.bus_path")
            .or_else(|| property("device.name"))
            .unwrap_or(source_name)
            .to_owned();
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
            id,
            name,
            source_name: source_name.to_owned(),
            bluetooth,
            legacy_id,
        };
        found.push((microphone, card));
    }
    disambiguate_identical_usb_devices(&mut found);

    let mut by_id: HashMap<String, DiscoveredMicrophone> = HashMap::new();
    let mut order = Vec::new();
    for (microphone, _) in found {
        let id = microphone.id.clone();
        let is_default = default_source.as_deref() == Some(microphone.source_name.as_str());
        match by_id.get_mut(&id) {
            Some(existing) if is_default => {
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

/// The source a capture process records from, as the sound server reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StreamSource {
    pub source_name: String,
    pub bluetooth: bool,
}

/// Looks up which source the capture process `pid` records from. `None` when
/// the server lists no stream for that process.
pub(crate) fn stream_source(pid: u32) -> Result<Option<StreamSource>> {
    let outputs = pactl_json(&["list", "source-outputs"])?;
    let Some(index) = source_index_for_process(&outputs, pid)? else {
        return Ok(None);
    };
    let sources = pactl_json(&["list", "sources"])?;
    source_by_index(&sources, index)
}

fn pactl_json(args: &[&str]) -> Result<Vec<u8>> {
    let what = args.join(" ");
    let output = bounded_output(
        Command::new("pactl").arg("--format=json").args(args),
        PACTL_TIMEOUT,
        PACTL_OUTPUT_LIMIT,
    )
    .with_context(|| format!("failed to run pactl {what}"))?;
    if output.timed_out {
        bail!("pactl {what} timed out after {PACTL_TIMEOUT:?}");
    }
    if output.stdout_truncated {
        bail!("pactl {what} output exceeded {PACTL_OUTPUT_LIMIT} bytes");
    }
    if !output.status.success() {
        bail!(
            "pactl {what} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn source_index_for_process(source_outputs: &[u8], pid: u32) -> Result<Option<u64>> {
    let outputs: Vec<Value> =
        serde_json::from_slice(source_outputs).context("failed to parse pactl stream data")?;
    let pid = pid.to_string();
    Ok(outputs
        .iter()
        .find(|output| {
            output
                .pointer("/properties/application.process.id")
                .and_then(Value::as_str)
                == Some(pid.as_str())
        })
        .and_then(|output| output.get("source"))
        .and_then(Value::as_u64))
}

fn source_by_index(sources: &[u8], index: u64) -> Result<Option<StreamSource>> {
    let sources: Vec<Value> =
        serde_json::from_slice(sources).context("failed to parse pactl microphone data")?;
    Ok(sources
        .iter()
        .find(|source| source.get("index").and_then(Value::as_u64) == Some(index))
        .and_then(|source| {
            let source_name = source.get("name").and_then(Value::as_str)?;
            let properties = source.get("properties").and_then(Value::as_object);
            Some(StreamSource {
                source_name: source_name.to_owned(),
                bluetooth: is_bluetooth(properties, source_name),
            })
        }))
}

fn is_bluetooth(properties: Option<&serde_json::Map<String, Value>>, source_name: &str) -> bool {
    let property = |key: &str| {
        properties
            .and_then(|values| values.get(key))
            .and_then(Value::as_str)
    };
    property("device.bus") == Some("bluetooth")
        || property("device.api") == Some("bluez5")
        || source_name.starts_with("bluez_")
}

/// Bluetooth status for a saved microphone that is not connected right now.
fn is_bluetooth_id(id: &str) -> bool {
    id.starts_with("bluetooth:") || id.contains("bluez_")
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
    if let Some(id) = usb_device_id(&property) {
        return id;
    }
    if let Some(path) = property("device.bus_path") {
        return format!("bus:{path}");
    }
    if let Some(device) = property("device.name") {
        return format!("device:{device}");
    }
    format!("source:{source_name}")
}

/// Identifies a USB device by what it reports about itself rather than the
/// port it is plugged into: vendor and product id, plus the serial when the
/// device has one. `None` when it reports neither ids nor a serial.
fn usb_device_id<'a>(property: &impl Fn(&str) -> Option<&'a str>) -> Option<String> {
    if property("device.bus") != Some("usb") {
        return None;
    }
    let hex = |key: &str| {
        property(key).map(|value| value.trim().trim_start_matches("0x").to_ascii_lowercase())
    };
    match (
        hex("device.vendor.id"),
        hex("device.product.id"),
        property("device.serial"),
    ) {
        (Some(vendor), Some(product), Some(serial)) => {
            Some(format!("{USB_ID_PREFIX}{vendor}:{product}:{serial}"))
        }
        (Some(vendor), Some(product), None) => Some(format!("{USB_ID_PREFIX}{vendor}:{product}")),
        (_, _, Some(serial)) => Some(format!("{USB_ID_PREFIX}serial:{serial}")),
        _ => None,
    }
}

/// Two identical devices plugged in at once report the same USB identity.
/// The card at the lowest bus path keeps the plain id; each other card gets
/// its bus path appended, so neither is dropped. Sources of one card (its
/// profiles) share a bus path and stay merged.
fn disambiguate_identical_usb_devices(found: &mut [(DiscoveredMicrophone, String)]) {
    let mut cards: HashMap<String, std::collections::BTreeSet<String>> = HashMap::new();
    for (microphone, card) in found.iter() {
        if microphone.id.starts_with(USB_ID_PREFIX) {
            cards
                .entry(microphone.id.clone())
                .or_default()
                .insert(card.clone());
        }
    }
    for (microphone, card) in found.iter_mut() {
        let Some(group) = cards.get(&microphone.id) else {
            continue;
        };
        if group.len() > 1 && group.first() != Some(card) {
            microphone.id = format!("{}@{card}", microphone.id);
        }
    }
}

/// When one of two identical devices is unplugged, the one left would take
/// the plain id and lose the rank saved under its `@<bus path>` id. A lone
/// device at a port with such a saved entry keeps that id instead.
fn keep_saved_discriminators(discovery: &Discovery, saved: &SavedPreferences) -> Discovery {
    let mut aligned = discovery.clone();
    let connected: HashSet<&str> = discovery
        .microphones
        .iter()
        .map(|microphone| microphone.id.as_str())
        .collect();
    for microphone in &mut aligned.microphones {
        let Some(path) = microphone
            .legacy_id
            .as_deref()
            .and_then(|legacy| legacy.strip_prefix("bus:"))
        else {
            continue;
        };
        if microphone.id.contains('@') {
            continue;
        }
        let discriminated = format!("{}@{path}", microphone.id);
        if !connected.contains(discriminated.as_str())
            && saved
                .microphones
                .iter()
                .any(|entry| entry.id == discriminated)
        {
            microphone.id = discriminated;
        }
    }
    aligned
}

/// A `bus:` id for a USB port, as saved before USB devices had port-independent ids.
fn is_legacy_usb_id(id: &str) -> bool {
    id.starts_with("bus:") && id.contains("-usb-")
}

/// Moves saved rankings from port-based USB ids to the current ids.
///
/// A connected USB device with no saved entry adopts the entry saved under its
/// own old bus path, or else the highest-ranked old USB entry with its name.
/// Old USB entries sharing a name with each other or with a device adopted
/// here are taken to be one device seen at several ports: they collapse into
/// the highest-ranked of them. A port id cannot tell that case from two
/// units of one model, so a second unit loses its old rank and is appended
/// when it is next connected. Running it again changes nothing.
fn migrate_usb_ids(saved: &mut SavedPreferences, discovery: &Discovery) -> bool {
    let current: HashSet<&str> = discovery
        .microphones
        .iter()
        .map(|microphone| microphone.id.as_str())
        .collect();
    let stale_legacy = |entry: &SavedMicrophone| {
        is_legacy_usb_id(&entry.id) && !current.contains(entry.id.as_str())
    };
    let mut adopted: HashSet<String> = HashSet::new();
    for device in &discovery.microphones {
        let Some(legacy_id) = device.legacy_id.as_deref() else {
            continue;
        };
        if saved.microphones.iter().any(|entry| entry.id == device.id) {
            continue;
        }
        let index = saved
            .microphones
            .iter()
            .position(|entry| entry.id == legacy_id && stale_legacy(entry))
            .or_else(|| {
                saved
                    .microphones
                    .iter()
                    .position(|entry| entry.name == device.name && stale_legacy(entry))
            });
        if let Some(index) = index {
            let entry = &mut saved.microphones[index];
            entry.id.clone_from(&device.id);
            entry.name.clone_from(&device.name);
            adopted.insert(device.id.clone());
        }
    }

    struct Group {
        slot: usize,
        has_adopted: bool,
    }
    let before = saved.microphones.len();
    let mut groups: HashMap<String, Group> = HashMap::new();
    let mut kept: Vec<SavedMicrophone> = Vec::with_capacity(before);
    for entry in saved.microphones.drain(..) {
        let is_adopted = adopted.contains(&entry.id);
        if !is_adopted && !stale_legacy(&entry) {
            kept.push(entry);
            continue;
        }
        match groups.get_mut(&entry.name) {
            Some(group) if !(is_adopted && group.has_adopted) => {
                if is_adopted {
                    kept[group.slot].id = entry.id;
                    group.has_adopted = true;
                }
            }
            Some(_) => kept.push(entry),
            None => {
                groups.insert(
                    entry.name.clone(),
                    Group {
                        slot: kept.len(),
                        has_adopted: is_adopted,
                    },
                );
                kept.push(entry);
            }
        }
    }
    saved.microphones = kept;
    !adopted.is_empty() || saved.microphones.len() != before
}

fn reconcile(saved: &mut SavedPreferences, discovery: &Discovery) -> bool {
    let discovered: HashMap<_, _> = discovery
        .microphones
        .iter()
        .map(|microphone| (microphone.id.as_str(), microphone))
        .collect();
    let mut changed = migrate_usb_ids(saved, discovery);
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
                    bluetooth: current.map_or_else(|| is_bluetooth_id(&saved.id), |m| m.bluetooth),
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
            "alsa_input.usb-mic.mono-fallback"
        );

        let unsaved = SavedPreferences::default();
        assert_eq!(
            select_microphone(&unsaved, &discovery).unwrap().source_name,
            "alsa_input.usb-mic.mono-fallback"
        );
    }

    #[test]
    fn bluetooth_inputs_are_flagged_and_never_selected() {
        let discovery = discovery_from_pactl_json(
            SOURCES.as_bytes(),
            Some("bluez_input.11_22_33.headset-head-unit".into()),
        )
        .unwrap();
        let flags: Vec<_> = discovery
            .microphones
            .iter()
            .map(|microphone| (microphone.source_name.as_str(), microphone.bluetooth))
            .collect();
        assert_eq!(
            flags,
            [
                ("bluez_input.11_22_33.headset-head-unit", true),
                ("alsa_input.usb-mic.mono-fallback", false),
            ]
        );

        // Preferred first and system default, but still skipped.
        let saved = SavedPreferences {
            microphones: vec![SavedMicrophone {
                id: "bluetooth:11:22:33:44:55:66".into(),
                name: "Travel Headset".into(),
            }],
            ..SavedPreferences::default()
        };
        assert_eq!(
            select_microphone(&saved, &discovery).unwrap().source_name,
            "alsa_input.usb-mic.mono-fallback"
        );
        assert_eq!(
            select_microphone(&SavedPreferences::default(), &discovery)
                .unwrap()
                .source_name,
            "alsa_input.usb-mic.mono-fallback"
        );
    }

    #[test]
    fn bluetooth_detection_covers_bus_api_and_source_name() {
        let properties = |value: serde_json::Value| {
            serde_json::from_value::<serde_json::Map<String, Value>>(value).unwrap()
        };
        assert!(is_bluetooth(
            Some(&properties(serde_json::json!({"device.bus": "bluetooth"}))),
            "alsa_input.renamed"
        ));
        assert!(is_bluetooth(
            Some(&properties(serde_json::json!({"device.api": "bluez5"}))),
            "renamed"
        ));
        assert!(is_bluetooth(None, "bluez_input.headset"));
        assert!(!is_bluetooth(
            Some(&properties(serde_json::json!({"device.bus": "usb"}))),
            "alsa_input.usb-mic"
        ));
    }

    #[test]
    fn stream_source_is_found_by_capture_process_id() {
        let outputs = br#"[
          {"index": 90, "source": 3, "properties": {"application.process.id": "4100"}},
          {"index": 91, "source": 5, "properties": {"application.process.id": "4242"}},
          {"index": 92, "source": 9, "properties": {}}
        ]"#;
        let sources = br#"[
          {"index": 3, "name": "alsa_input.usb-mic", "properties": {"device.bus": "usb"}},
          {"index": 5, "name": "headset.renamed", "properties": {"device.bus": "bluetooth"}}
        ]"#;
        let located = |pid| {
            source_index_for_process(outputs, pid)
                .unwrap()
                .map(|index| source_by_index(sources, index).unwrap())
        };

        assert_eq!(
            located(4100),
            Some(Some(StreamSource {
                source_name: "alsa_input.usb-mic".into(),
                bluetooth: false,
            }))
        );
        assert_eq!(
            located(4242),
            Some(Some(StreamSource {
                source_name: "headset.renamed".into(),
                bluetooth: true,
            }))
        );
        assert_eq!(located(7), None);
        assert_eq!(source_by_index(sources, 9).unwrap(), None);
        assert!(source_index_for_process(b"not json", 1).is_err());
    }

    #[test]
    fn only_bluetooth_inputs_fail_with_a_clear_error() {
        let only_bluetooth = r#"[{
            "name": "bluez_input.11_22_33.headset-head-unit",
            "description": "Travel Headset",
            "monitor_source": "",
            "properties": {"device.bus": "bluetooth", "device.string": "11:22:33:44:55:66"}
        }]"#;
        let directory = tempfile::tempdir().unwrap();
        let manager = DeviceManager::at(directory.path().join(PREFERENCES_FILE));
        let discovery = discovery_from_pactl_json(only_bluetooth.as_bytes(), None).unwrap();
        assert_eq!(
            manager.resolve(&discovery).unwrap_err().to_string(),
            "no non-Bluetooth microphone available"
        );

        let nothing = discovery_from_pactl_json(b"[]", None).unwrap();
        assert!(
            manager
                .resolve(&nothing)
                .unwrap_err()
                .to_string()
                .contains("no connected input device")
        );
    }

    #[test]
    fn preference_view_marks_connected_and_saved_bluetooth_inputs() {
        let saved = SavedPreferences {
            microphones: vec![
                SavedMicrophone {
                    id: "bluetooth:11:22:33:44:55:66".into(),
                    name: "Travel Headset".into(),
                },
                SavedMicrophone {
                    id: "bluetooth:aa:bb:cc:dd:ee:ff".into(),
                    name: "Disconnected earbuds".into(),
                },
                SavedMicrophone {
                    id: "bus:pci-0000:00:14.0-usb-0:5:1.0".into(),
                    name: "Desk microphone".into(),
                },
            ],
            ..SavedPreferences::default()
        };
        let view = view_preferences(&saved, &discovery());
        let flags: Vec<_> = view
            .microphones
            .iter()
            .map(|microphone| (microphone.connected, microphone.bluetooth))
            .collect();
        assert_eq!(flags, [(true, true), (false, true), (true, false)]);
    }

    #[test]
    fn resolution_reports_the_preferences_version_it_wrote() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(PREFERENCES_FILE);
        let manager = DeviceManager::at(path.clone());

        // The first resolution seeds the file.
        let seeded = manager.resolve(&discovery()).unwrap();
        assert!(seeded.fingerprint.is_some());
        assert_eq!(seeded.fingerprint, fingerprint_at(&path));

        // Nothing new to record: the version is unchanged.
        let again = manager.resolve(&discovery()).unwrap();
        assert_eq!(again.fingerprint, seeded.fingerprint);
    }

    #[test]
    fn fingerprint_changes_when_preferences_are_rewritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(PREFERENCES_FILE);
        assert_eq!(fingerprint_at(&path), None);

        let mut saved = SavedPreferences::default();
        reconcile(&mut saved, &discovery());
        write_preferences(&path, &saved).unwrap();
        let first = fingerprint_at(&path).unwrap();
        assert_eq!(fingerprint_at(&path), Some(first));

        saved.microphones.reverse();
        write_preferences(&path, &saved).unwrap();
        assert_ne!(fingerprint_at(&path), Some(first));
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

    /// The user's saved list after one webcam and one microphone were each
    /// plugged into three different USB ports.
    const LEGACY_PREFERENCES: &str = r#"{
      "version": 1,
      "microphones": [
        {"id": "bus:pci-0000:c3:00.3-usb-0:1.3.1:1.2", "name": "Webcam C270 Mono"},
        {"id": "bus:pci-0000:c3:00.4-usb-0:1.4:1.0", "name": "BY-GM18CU Mono"},
        {"id": "bus:pci-0000:c3:00.4-usb-0:1.3.2:1.2", "name": "Webcam C270 Mono"},
        {"id": "bus:pci-0000:c1:00.3-usb-0:2:1.0", "name": "BY-GM18CU Mono"},
        {"id": "bus:pci-0000:c3:00.3-usb-0:1.4:1.0", "name": "BY-GM18CU Mono"},
        {"id": "bus:pci-0000:c1:00.6", "name": "Ryzen HD Audio Controller Analog Stereo"},
        {"id": "bluetooth:88:C9:E8:16:C7:9C", "name": "WH-1000XM4"},
        {"id": "bus:pci-0000:c3:00.3-usb-0:1.3.2:1.2", "name": "Webcam C270 Mono"}
      ]
    }"#;

    const C270_ID: &str = "usb:046d:0825:046d_C270_HD_WEBCAM_4F2C0660";

    fn ryzen_source() -> Value {
        serde_json::json!({
            "name": "alsa_input.pci-0000_c1_00.6.analog-stereo",
            "description": "Ryzen HD Audio Controller Analog Stereo",
            "monitor_source": "",
            "properties": {
                "device.bus": "pci",
                "device.bus_path": "pci-0000:c1:00.6",
                "device.vendor.id": "0x1022",
                "device.product.id": "0x15e3",
                "device.name": "alsa_card.pci-0000_c1_00.6"
            }
        })
    }

    fn c270_source(bus_path: &str, serial: Option<&str>) -> Value {
        let mut properties = serde_json::json!({
            "device.bus": "usb",
            "device.bus_path": bus_path,
            "device.vendor.id": "0x046d",
            "device.product.id": "0x0825",
            "device.name": "alsa_card.usb-046d_C270_HD_WEBCAM_4F2C0660-02"
        });
        if let Some(serial) = serial {
            properties["device.serial"] = serial.into();
        }
        serde_json::json!({
            "name": format!("alsa_input.usb-046d_C270-{bus_path}.mono-fallback"),
            "description": "Webcam C270 Mono",
            "monitor_source": "",
            "properties": properties
        })
    }

    fn by_source(bus_path: &str) -> Value {
        serde_json::json!({
            "name": "alsa_input.usb-BY-GM18CU.mono-fallback",
            "description": "BY-GM18CU Mono",
            "monitor_source": "",
            "properties": {
                "device.bus": "usb",
                "device.bus_path": bus_path,
                "device.vendor.id": "0x0d8c",
                "device.product.id": "0x0014",
                "device.name": "alsa_card.usb-BY-GM18CU-00"
            }
        })
    }

    fn discovery_of(sources: &[Value]) -> Discovery {
        discovery_from_pactl_json(&serde_json::to_vec(sources).unwrap(), None).unwrap()
    }

    fn saved_entries(saved: &SavedPreferences) -> Vec<(&str, &str)> {
        saved
            .microphones
            .iter()
            .map(|microphone| (microphone.id.as_str(), microphone.name.as_str()))
            .collect()
    }

    #[test]
    fn usb_devices_get_port_independent_ids() {
        let id = |properties: serde_json::Value| {
            let properties =
                serde_json::from_value::<serde_json::Map<String, Value>>(properties).unwrap();
            stable_device_id(Some(&properties), "alsa_input.source")
        };
        let cases = [
            (
                "vendor, product and serial",
                serde_json::json!({
                    "device.bus": "usb",
                    "device.bus_path": "pci-0000:c3:00.3-usb-0:1.3.1:1.2",
                    "device.serial": "046d_C270_HD_WEBCAM_4F2C0660",
                    "device.vendor.id": "0x046d",
                    "device.product.id": "0x0825",
                    "device.name": "alsa_card.usb-046d_C270_HD_WEBCAM_4F2C0660-02"
                }),
                C270_ID,
            ),
            (
                "no serial",
                serde_json::json!({
                    "device.bus": "usb",
                    "device.bus_path": "pci-0000:c3:00.4-usb-0:1.4:1.0",
                    "device.vendor.id": "0x0D8C",
                    "device.product.id": "0x0014"
                }),
                "usb:0d8c:0014",
            ),
            (
                "serial only",
                serde_json::json!({
                    "device.bus": "usb",
                    "device.bus_path": "pci-0000:c3:00.4-usb-0:1.4:1.0",
                    "device.serial": "ABC123"
                }),
                "usb:serial:ABC123",
            ),
            (
                "usb without identity keeps the bus path",
                serde_json::json!({
                    "device.bus": "usb",
                    "device.bus_path": "pci-0000:c3:00.4-usb-0:1.4:1.0",
                    "device.vendor.id": "0x0d8c"
                }),
                "bus:pci-0000:c3:00.4-usb-0:1.4:1.0",
            ),
            (
                "pci keeps the bus path",
                serde_json::json!({
                    "device.bus": "pci",
                    "device.bus_path": "pci-0000:c1:00.6",
                    "device.vendor.id": "0x1022",
                    "device.product.id": "0x15e3"
                }),
                "bus:pci-0000:c1:00.6",
            ),
            (
                "bluetooth is unchanged",
                serde_json::json!({
                    "device.bus": "bluetooth",
                    "device.string": "88:C9:E8:16:C7:9C",
                    "device.vendor.id": "0x054c"
                }),
                "bluetooth:88:C9:E8:16:C7:9C",
            ),
        ];
        for (name, properties, expected) in cases {
            assert_eq!(id(properties), expected, "{name}");
        }
    }

    #[test]
    fn legacy_usb_entries_migrate_without_losing_rank() {
        let mut saved: SavedPreferences = serde_json::from_str(LEGACY_PREFERENCES).unwrap();
        let connected = discovery_of(&[
            ryzen_source(),
            c270_source(
                "pci-0000:c3:00.3-usb-0:1.3.1:1.2",
                Some("046d_C270_HD_WEBCAM_4F2C0660"),
            ),
        ]);

        assert!(reconcile(&mut saved, &connected));
        let expected = [
            (C270_ID, "Webcam C270 Mono"),
            ("bus:pci-0000:c3:00.4-usb-0:1.4:1.0", "BY-GM18CU Mono"),
            (
                "bus:pci-0000:c1:00.6",
                "Ryzen HD Audio Controller Analog Stereo",
            ),
            ("bluetooth:88:C9:E8:16:C7:9C", "WH-1000XM4"),
        ];
        assert_eq!(saved_entries(&saved), expected);
        assert!(!reconcile(&mut saved, &connected), "migration runs once");

        // The microphone comes back on yet another port.
        let plugged = discovery_of(&[
            ryzen_source(),
            c270_source(
                "pci-0000:c3:00.3-usb-0:1.3.1:1.2",
                Some("046d_C270_HD_WEBCAM_4F2C0660"),
            ),
            by_source("pci-0000:c1:00.4-usb-0:3:1.0"),
        ]);
        assert!(reconcile(&mut saved, &plugged));
        assert_eq!(saved.microphones[1].id, "usb:0d8c:0014");
        assert_eq!(saved.microphones.len(), 4);
        assert!(!reconcile(&mut saved, &plugged));
    }

    #[test]
    fn adoption_prefers_the_old_bus_path_but_keeps_the_highest_rank() {
        let mut saved: SavedPreferences = serde_json::from_str(LEGACY_PREFERENCES).unwrap();
        // Plugged into the port of the second-ranked C270 entry.
        let connected = discovery_of(&[c270_source(
            "pci-0000:c3:00.4-usb-0:1.3.2:1.2",
            Some("046d_C270_HD_WEBCAM_4F2C0660"),
        )]);

        assert!(reconcile(&mut saved, &connected));
        let ids: Vec<_> = saved.microphones.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                C270_ID,
                "bus:pci-0000:c3:00.4-usb-0:1.4:1.0",
                "bus:pci-0000:c1:00.6",
                "bluetooth:88:C9:E8:16:C7:9C",
            ]
        );
    }

    #[test]
    fn migration_writes_once_and_keeps_the_resolution_fingerprint() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(PREFERENCES_FILE);
        fs::write(&path, LEGACY_PREFERENCES).unwrap();
        let manager = DeviceManager::at(path.clone());
        let connected = discovery_of(&[
            ryzen_source(),
            c270_source(
                "pci-0000:c3:00.3-usb-0:1.3.1:1.2",
                Some("046d_C270_HD_WEBCAM_4F2C0660"),
            ),
        ]);

        let migrated = manager.resolve(&connected).unwrap();
        assert_eq!(migrated.microphone.id, C270_ID);
        assert_eq!(migrated.fingerprint, fingerprint_at(&path));
        assert_eq!(
            read_preferences(&path).unwrap().unwrap().microphones.len(),
            4
        );

        let again = manager.resolve(&connected).unwrap();
        assert_eq!(again.fingerprint, migrated.fingerprint);
    }

    #[test]
    fn identical_usb_devices_connected_together_stay_distinct() {
        let serial = Some("046d_C270_HD_WEBCAM_4F2C0660");
        let cases = [("same serial", serial), ("no serial", None)];
        for (name, serial) in cases {
            // A second profile of the first card is the same device.
            let mut second_profile = c270_source("pci-0000:c3:00.3-usb-0:1.3.1:1.2", serial);
            second_profile["name"] = "alsa_input.usb-046d_C270.analog-stereo".into();
            let discovery = discovery_of(&[
                c270_source("pci-0000:c3:00.4-usb-0:1.3.2:1.2", serial),
                c270_source("pci-0000:c3:00.3-usb-0:1.3.1:1.2", serial),
                second_profile,
            ]);
            let base = match serial {
                Some(serial) => format!("usb:046d:0825:{serial}"),
                None => "usb:046d:0825".to_string(),
            };
            let mut ids: Vec<_> = discovery
                .microphones
                .iter()
                .map(|microphone| microphone.id.clone())
                .collect();
            ids.sort();
            let expected = vec![
                base.clone(),
                format!("{base}@pci-0000:c3:00.4-usb-0:1.3.2:1.2"),
            ];
            assert_eq!(ids, expected, "{name}");
        }

        // Both adopt their own legacy entries and neither is collapsed.
        let mut saved: SavedPreferences = serde_json::from_str(LEGACY_PREFERENCES).unwrap();
        let both = discovery_of(&[
            c270_source("pci-0000:c3:00.3-usb-0:1.3.1:1.2", serial),
            c270_source("pci-0000:c3:00.4-usb-0:1.3.2:1.2", serial),
        ]);
        assert!(reconcile(&mut saved, &both));
        let webcams: Vec<_> = saved
            .microphones
            .iter()
            .filter(|microphone| microphone.name == "Webcam C270 Mono")
            .map(|microphone| microphone.id.clone())
            .collect();
        assert_eq!(
            webcams,
            [
                C270_ID.to_string(),
                format!("{C270_ID}@pci-0000:c3:00.4-usb-0:1.3.2:1.2"),
            ]
        );
        assert!(!reconcile(&mut saved, &both));
    }

    #[test]
    fn an_identical_device_keeps_its_rank_when_its_twin_is_unplugged() {
        let (first, second) = (
            "pci-0000:c3:00.3-usb-0:1.3.1:1.2",
            "pci-0000:c3:00.4-usb-0:1.3.2:1.2",
        );
        let second_id = format!("usb:046d:0825@{second}");
        let saved = SavedPreferences {
            microphones: vec![
                SavedMicrophone {
                    id: second_id.clone(),
                    name: "Webcam C270 Mono".into(),
                },
                SavedMicrophone {
                    id: "bus:pci-0000:c1:00.6".into(),
                    name: "Built-in".into(),
                },
                SavedMicrophone {
                    id: "usb:046d:0825".into(),
                    name: "Webcam C270 Mono".into(),
                },
            ],
            ..SavedPreferences::default()
        };
        let cases = [
            ("twin unplugged", vec![second], vec![second_id.as_str()]),
            ("other one unplugged", vec![first], vec!["usb:046d:0825"]),
            (
                "both connected",
                vec![first, second],
                vec!["usb:046d:0825", second_id.as_str()],
            ),
        ];
        for (name, ports, expected) in cases {
            let sources: Vec<_> = ports.iter().map(|port| c270_source(port, None)).collect();
            let aligned = keep_saved_discriminators(&discovery_of(&sources), &saved);
            let mut ids: Vec<_> = aligned
                .microphones
                .iter()
                .map(|microphone| microphone.id.as_str())
                .collect();
            ids.sort();
            assert_eq!(ids, expected, "{name}");
            let mut reconciled = saved.clone();
            assert!(!reconcile(&mut reconciled, &aligned), "{name}");
        }
    }

    #[test]
    fn remove_microphone_drops_disconnected_entries_only() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(PREFERENCES_FILE);
        fs::write(&path, LEGACY_PREFERENCES).unwrap();
        let manager = DeviceManager::at(path.clone());
        let connected = discovery_of(&[
            ryzen_source(),
            c270_source(
                "pci-0000:c3:00.3-usb-0:1.3.1:1.2",
                Some("046d_C270_HD_WEBCAM_4F2C0660"),
            ),
        ]);

        let view = manager
            .remove("bus:pci-0000:c3:00.4-usb-0:1.4:1.0", &connected)
            .unwrap();
        let names: Vec<_> = view.microphones.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Webcam C270 Mono",
                "Ryzen HD Audio Controller Analog Stereo",
                "WH-1000XM4",
            ]
        );
        assert_eq!(
            read_preferences(&path).unwrap().unwrap().microphones.len(),
            3
        );

        let before = fs::read(&path).unwrap();
        let connected_error = manager.remove(C270_ID, &connected).unwrap_err().to_string();
        assert_eq!(
            connected_error,
            "\"Webcam C270 Mono\" is connected; disconnect it before removing it, or it is added back on the next discovery"
        );
        let unknown_error = manager
            .remove("bus:gone", &connected)
            .unwrap_err()
            .to_string();
        assert_eq!(unknown_error, "no saved microphone has the id \"bus:gone\"");
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "failed removals do not write"
        );
    }

    #[test]
    fn hung_pactl_command_is_killed_on_the_discovery_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("hung-pactl");
        fs::write(&executable, "#!/bin/sh\nwhile :; do :; done\n").unwrap();
        let started = std::time::Instant::now();

        let error = discover_microphones_with(
            || {
                let mut command = Command::new("/bin/sh");
                command.arg(&executable);
                command
            },
            Duration::from_millis(100),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
