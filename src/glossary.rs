//! The user's vocabulary glossary: terms sent in the `vocabulary` form field
//! of a transcription request so the server (siren) boosts their spelling.
//!
//! The file lives in the data directory, because Home Manager owns
//! `config.json` and the desktop app and CLI must be able to edit the list:
//!
//! ```json
//! {"version": 1, "terms": ["Claude", "tmux"]}
//! ```
//!
//! A missing file sends no `vocabulary` field, so the server uses its default
//! list. A file with an empty `terms` list sends `[]`, which turns boosting off.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use tracing::warn;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

use crate::utils::{DATA_DIR, create_private_dir};

pub const GLOSSARY_VERSION: u32 = 1;
/// siren rejects a `vocabulary` field with more terms than this.
pub const MAX_TERMS: usize = 200;
/// siren rejects a term longer than this many characters after trimming.
pub const MAX_TERM_CHARS: usize = 64;
const GLOSSARY_FILE: &str = "glossary.json";
const LOCK_FILE: &str = ".glossary.lock";

/// `$XDG_DATA_HOME/dictator/glossary.json`
pub fn glossary_path() -> PathBuf {
    DATA_DIR.join(GLOSSARY_FILE)
}

#[derive(Debug, Serialize, Deserialize)]
struct GlossaryFile {
    version: u32,
    terms: Vec<String>,
}

/// Trims `raw` and checks it against siren's per-term rules.
pub fn normalize_term(raw: &str) -> Result<String, String> {
    let term = raw.trim();
    if term.is_empty() {
        return Err("A term cannot be empty".to_string());
    }
    let length = term.chars().count();
    if length > MAX_TERM_CHARS {
        return Err(format!(
            "A term can have at most {MAX_TERM_CHARS} characters; this one has {length}"
        ));
    }
    if !term.chars().all(is_printable) {
        return Err("A term cannot contain control or invisible characters".to_string());
    }
    Ok(term.to_string())
}

/// Mirrors Python's `str.isprintable`, which siren applies to every term.
fn is_printable(c: char) -> bool {
    c == ' '
        || !matches!(
            c.general_category(),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::Surrogate
                | GeneralCategory::PrivateUse
                | GeneralCategory::Unassigned
                | GeneralCategory::SpaceSeparator
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        )
}

fn same_term(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Returns the terms siren will accept, in order: each trimmed, invalid
/// entries and case-insensitive duplicates dropped, capped at [`MAX_TERMS`].
/// The second value explains each dropped entry.
pub fn sanitize(terms: &[String]) -> (Vec<String>, Vec<String>) {
    let mut kept: Vec<String> = Vec::new();
    let mut dropped = Vec::new();
    for raw in terms {
        match normalize_term(raw) {
            Err(reason) => dropped.push(format!("{raw:?}: {reason}")),
            Ok(term) if kept.iter().any(|seen| same_term(seen, &term)) => {
                dropped.push(format!("{raw:?}: duplicate"));
            }
            Ok(_) if kept.len() == MAX_TERMS => {
                dropped.push(format!("{raw:?}: over the {MAX_TERMS}-term limit"));
            }
            Ok(term) => kept.push(term),
        }
    }
    (kept, dropped)
}

/// Appends a term. Fails with a message fit for the user when the term is
/// invalid, already present (ignoring case), or the list is full.
pub fn insert_term(terms: &mut Vec<String>, raw: &str) -> Result<(), String> {
    let term = normalize_term(raw)?;
    if terms.iter().any(|existing| same_term(existing, &term)) {
        return Err(format!("\"{term}\" is already in the glossary"));
    }
    if terms.len() >= MAX_TERMS {
        return Err(format!("The glossary is full ({MAX_TERMS} terms)"));
    }
    terms.push(term);
    Ok(())
}

/// Removes every entry that matches `raw`, ignoring case and surrounding
/// whitespace.
pub fn delete_term(terms: &mut Vec<String>, raw: &str) -> Result<(), String> {
    let before = terms.len();
    terms.retain(|existing| !same_term(existing, raw));
    if terms.len() == before {
        return Err(format!("\"{}\" is not in the glossary", raw.trim()));
    }
    Ok(())
}

/// Replaces the first entry matching `from` (ignoring case and surrounding
/// whitespace) with `to`, keeping its position. `to` may differ from `from`
/// only in case, but must not duplicate another term. Other entries matching
/// `from` are duplicates and are dropped.
pub fn replace_term(terms: &mut Vec<String>, from: &str, to: &str) -> Result<(), String> {
    let to = normalize_term(to)?;
    let Some(index) = terms.iter().position(|existing| same_term(existing, from)) else {
        return Err(format!("\"{}\" is not in the glossary", from.trim()));
    };
    if terms
        .iter()
        .any(|existing| !same_term(existing, from) && same_term(existing, &to))
    {
        return Err(format!("\"{to}\" is already in the glossary"));
    }
    *terms = std::mem::take(terms)
        .into_iter()
        .enumerate()
        .filter_map(|(position, existing)| {
            if position == index {
                Some(to.clone())
            } else if same_term(&existing, from) {
                None
            } else {
                Some(existing)
            }
        })
        .collect();
    Ok(())
}

/// Reads the stored terms as written. `None` means the file does not exist.
pub fn load(path: &Path) -> Result<Option<Vec<String>>> {
    match File::open(path) {
        Ok(file) => read_open(path, file).map(Some),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn read_open(path: &Path, mut file: File) -> Result<Vec<String>> {
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let glossary: GlossaryFile = serde_json::from_slice(&data)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if glossary.version != GLOSSARY_VERSION {
        bail!(
            "unsupported glossary version {} in {}",
            glossary.version,
            path.display()
        );
    }
    Ok(glossary.terms)
}

/// Adds a term to the file, creating it if needed, and returns the new list.
pub fn add(path: &Path, raw: &str) -> Result<Vec<String>> {
    edit(path, |terms| insert_term(terms, raw))
}

/// Removes a term from the file and returns the new list. Removing the last
/// term leaves an empty list, which turns boosting off.
pub fn remove(path: &Path, raw: &str) -> Result<Vec<String>> {
    edit(path, |terms| delete_term(terms, raw))
}

/// Renames a term in the file and returns the new list.
pub fn rename(path: &Path, from: &str, to: &str) -> Result<Vec<String>> {
    edit(path, |terms| replace_term(terms, from, to))
}

fn edit(
    path: &Path,
    change: impl FnOnce(&mut Vec<String>) -> Result<(), String>,
) -> Result<Vec<String>> {
    let _lock = lock(path)?;
    let mut terms = load(path)?.unwrap_or_default();
    change(&mut terms).map_err(|message| anyhow!(message))?;
    write(path, &terms)?;
    Ok(terms)
}

/// Serializes read-modify-write cycles between the desktop app and the CLI.
struct GlossaryLock(File);

impl Drop for GlossaryLock {
    fn drop(&mut self) {
        // Closing the file releases the lock even if this unlock fails.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn parent_dir(path: &Path) -> Result<&Path> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("glossary path has no parent: {}", path.display()))?;
    create_private_dir(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    Ok(parent)
}

fn lock(path: &Path) -> Result<GlossaryLock> {
    let lock_path = parent_dir(path)?.join(LOCK_FILE);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .with_context(|| format!("failed to open {}", lock_path.display()))?;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(GlossaryLock(file));
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(err).with_context(|| format!("failed to lock {}", lock_path.display()));
        }
    }
}

/// Replaces the file atomically, so the daemon never reads a partial list.
fn write(path: &Path, terms: &[String]) -> Result<()> {
    let parent = parent_dir(path)?;
    let temp_path = parent.join(format!(
        ".{GLOSSARY_FILE}.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let glossary = GlossaryFile {
        version: GLOSSARY_VERSION,
        terms: terms.to_vec(),
    };
    let result = (|| -> Result<()> {
        let mut temp = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp_path)
            .with_context(|| format!("failed to create {}", temp_path.display()))?;
        serde_json::to_writer_pretty(&mut temp, &glossary)
            .context("failed to serialize the glossary")?;
        temp.write_all(b"\n")
            .and_then(|()| temp.sync_all())
            .context("failed to write the glossary")?;
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

/// Identifies one version of the file: an atomic replace changes the inode,
/// an in-place edit changes the length or modification time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint {
    inode: u64,
    len: u64,
    modified_ns: i128,
}

impl Fingerprint {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            inode: metadata.ino(),
            len: metadata.len(),
            modified_ns: i128::from(metadata.mtime()) * 1_000_000_000
                + i128::from(metadata.mtime_nsec()),
        }
    }
}

/// Supplies the list to send with each request, re-reading the file only
/// when it changed.
pub struct Vocabulary {
    path: PathBuf,
    cached: Mutex<Option<(Fingerprint, Option<Vec<String>>)>>,
}

impl Vocabulary {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            cached: Mutex::new(None),
        }
    }

    /// `None` means "omit the field". A file that cannot be read or parsed
    /// is logged and treated like a missing one, so a bad edit falls back to
    /// the server's default list instead of failing every dictation.
    pub fn current(&self) -> Option<Vec<String>> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    warn!(path = %self.path.display(), %err, "cannot open glossary; sending no vocabulary");
                }
                *self.cached.lock().unwrap() = None;
                return None;
            }
        };
        let fingerprint = match file.metadata() {
            Ok(metadata) => Fingerprint::of(&metadata),
            Err(err) => {
                warn!(path = %self.path.display(), %err, "cannot stat glossary; sending no vocabulary");
                return None;
            }
        };
        let mut cached = self.cached.lock().unwrap();
        if let Some((seen, terms)) = cached.as_ref()
            && *seen == fingerprint
        {
            return terms.clone();
        }
        let terms = match read_open(&self.path, file) {
            Ok(stored) => {
                let (terms, dropped) = sanitize(&stored);
                for entry in dropped {
                    warn!(entry = %entry, "dropping glossary entry");
                }
                Some(terms)
            }
            Err(err) => {
                warn!(err = %format!("{err:#}"), "ignoring glossary; sending no vocabulary");
                None
            }
        };
        *cached = Some((fingerprint, terms.clone()));
        terms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn normalize_term_trims_and_enforces_siren_rules() {
        let cases: &[(&str, Result<&str, &str>)] = &[
            ("  tmux \t", Ok("tmux")),
            ("Claude Code", Ok("Claude Code")),
            ("Ångström", Ok("Ångström")),
            ("", Err("A term cannot be empty")),
            ("   ", Err("A term cannot be empty")),
            (
                "a\tb",
                Err("A term cannot contain control or invisible characters"),
            ),
            (
                "non\u{a0}breaking",
                Err("A term cannot contain control or invisible characters"),
            ),
            (
                "zero\u{200b}width",
                Err("A term cannot contain control or invisible characters"),
            ),
            (
                "private\u{e000}",
                Err("A term cannot contain control or invisible characters"),
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_term(input),
                expected.map(str::to_string).map_err(str::to_string),
                "{input:?}"
            );
        }

        let longest = "é".repeat(MAX_TERM_CHARS);
        assert_eq!(normalize_term(&longest), Ok(longest.clone()));
        assert_eq!(
            normalize_term(&format!("{longest}x")),
            Err(format!(
                "A term can have at most {MAX_TERM_CHARS} characters; this one has {}",
                MAX_TERM_CHARS + 1
            ))
        );
    }

    #[test]
    fn sanitize_drops_invalid_duplicate_and_excess_terms() {
        let mut terms = strings(&[" Claude ", "claude", "", "tmux", "a\u{7}b"]);
        terms.extend((0..MAX_TERMS).map(|index| format!("term{index}")));

        let (kept, dropped) = sanitize(&terms);

        assert_eq!(kept.len(), MAX_TERMS);
        assert_eq!(kept[..3], strings(&["Claude", "tmux", "term0"]));
        assert_eq!(kept.last().unwrap(), &format!("term{}", MAX_TERMS - 3));
        assert_eq!(dropped.len(), 3 + 2);
        assert!(dropped[0].contains("duplicate"), "{dropped:?}");
        assert!(dropped[1].contains("empty"), "{dropped:?}");
        assert!(dropped[2].contains("control"), "{dropped:?}");
        assert!(dropped[3].contains("limit"), "{dropped:?}");
    }

    #[test]
    fn insert_and_delete_report_user_facing_errors() {
        let mut terms = strings(&["Claude"]);
        assert_eq!(insert_term(&mut terms, " tmux "), Ok(()));
        assert_eq!(
            insert_term(&mut terms, "CLAUDE"),
            Err("\"CLAUDE\" is already in the glossary".to_string())
        );
        assert_eq!(terms, strings(&["Claude", "tmux"]));

        assert_eq!(delete_term(&mut terms, " claude"), Ok(()));
        assert_eq!(
            delete_term(&mut terms, "claude"),
            Err("\"claude\" is not in the glossary".to_string())
        );
        assert_eq!(terms, strings(&["tmux"]));

        let mut full: Vec<String> = (0..MAX_TERMS).map(|index| format!("t{index}")).collect();
        assert_eq!(
            insert_term(&mut full, "one more"),
            Err(format!("The glossary is full ({MAX_TERMS} terms)"))
        );
    }

    #[test]
    fn replace_term_keeps_position_and_rejects_conflicts() {
        /// name, stored, from, to, expected list or error
        type Case = (
            &'static str,
            &'static [&'static str],
            &'static str,
            &'static str,
            Result<&'static [&'static str], &'static str>,
        );
        let cases: &[Case] = &[
            (
                "keeps position",
                &["Claude", "tmux", "Sietch"],
                "tmux",
                " Zellij ",
                Ok(&["Claude", "Zellij", "Sietch"]),
            ),
            (
                "case-only change",
                &["Claude", "tmux"],
                "TMUX",
                "Tmux",
                Ok(&["Claude", "Tmux"]),
            ),
            (
                "unchanged",
                &["Claude"],
                "claude",
                "Claude",
                Ok(&["Claude"]),
            ),
            (
                "hand-edited duplicates collapse",
                &["claude", "tmux", "CLAUDE"],
                "Claude",
                "Claude",
                Ok(&["Claude", "tmux"]),
            ),
            (
                "duplicates another term",
                &["Claude", "tmux"],
                "tmux",
                "claude",
                Err("\"claude\" is already in the glossary"),
            ),
            (
                "missing term",
                &["Claude"],
                " zellij ",
                "Zellij",
                Err("\"zellij\" is not in the glossary"),
            ),
            (
                "invalid replacement",
                &["Claude"],
                "Claude",
                "  ",
                Err("A term cannot be empty"),
            ),
        ];
        for (name, stored, from, to, expected) in cases {
            let mut terms = strings(stored);
            let result = replace_term(&mut terms, from, to);
            match expected {
                Ok(list) => {
                    assert_eq!(result, Ok(()), "{name}");
                    assert_eq!(terms, strings(list), "{name}");
                }
                Err(message) => {
                    assert_eq!(result, Err(message.to_string()), "{name}");
                    assert_eq!(terms, strings(stored), "{name}: list unchanged");
                }
            }
        }
    }

    #[test]
    fn rename_rewrites_the_file_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(GLOSSARY_FILE);
        add(&path, "Claude").unwrap();
        add(&path, "tmux").unwrap();

        assert_eq!(
            rename(&path, "claude", "Claude Code").unwrap(),
            strings(&["Claude Code", "tmux"])
        );
        assert_eq!(
            load(&path).unwrap(),
            Some(strings(&["Claude Code", "tmux"]))
        );
        assert!(rename(&path, "tmux", "claude code").is_err());
        assert!(rename(dir.path().join("missing.json").as_path(), "a", "b").is_err());
    }

    #[test]
    fn file_round_trip_keeps_order_and_writes_privately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dictator").join(GLOSSARY_FILE);

        assert_eq!(load(&path).unwrap(), None);
        assert_eq!(add(&path, "Sietch").unwrap(), strings(&["Sietch"]));
        assert_eq!(
            add(&path, "Claude").unwrap(),
            strings(&["Sietch", "Claude"])
        );
        assert_eq!(load(&path).unwrap(), Some(strings(&["Sietch", "Claude"])));

        let stored: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            stored,
            serde_json::json!({"version": GLOSSARY_VERSION, "terms": ["Sietch", "Claude"]})
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        assert!(add(&path, "sietch").is_err());
        assert_eq!(remove(&path, "SIETCH").unwrap(), strings(&["Claude"]));
        assert_eq!(remove(&path, "Claude").unwrap(), Vec::<String>::new());
        assert_eq!(load(&path).unwrap(), Some(Vec::new()));
    }

    #[test]
    fn load_rejects_unknown_versions_and_corrupt_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(GLOSSARY_FILE);

        fs::write(&path, br#"{"version": 2, "terms": []}"#).unwrap();
        assert!(
            load(&path)
                .unwrap_err()
                .to_string()
                .contains("unsupported glossary version 2")
        );
        assert!(add(&path, "x").is_err(), "an edit must not reset the file");
        assert_eq!(fs::read(&path).unwrap(), br#"{"version": 2, "terms": []}"#);

        fs::write(&path, b"not json").unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn vocabulary_distinguishes_missing_from_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(GLOSSARY_FILE);
        let vocabulary = Vocabulary::new(path.clone());

        assert_eq!(vocabulary.current(), None);

        write(&path, &[]).unwrap();
        assert_eq!(vocabulary.current(), Some(Vec::new()));

        fs::remove_file(&path).unwrap();
        assert_eq!(vocabulary.current(), None);
    }

    #[test]
    fn vocabulary_reloads_after_each_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(GLOSSARY_FILE);
        let vocabulary = Vocabulary::new(path.clone());

        add(&path, "Claude").unwrap();
        assert_eq!(vocabulary.current(), Some(strings(&["Claude"])));
        assert_eq!(vocabulary.current(), Some(strings(&["Claude"])));

        add(&path, "tmux").unwrap();
        assert_eq!(vocabulary.current(), Some(strings(&["Claude", "tmux"])));

        // A hand edit in place, keeping the inode, is also picked up.
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        file.write_all(br#"{"version": 1, "terms": ["Sietch", "sietch", ""]}"#)
            .unwrap();
        drop(file);
        assert_eq!(vocabulary.current(), Some(strings(&["Sietch"])));

        fs::write(&path, b"{").unwrap();
        assert_eq!(vocabulary.current(), None, "corrupt file sends no field");
    }
}
