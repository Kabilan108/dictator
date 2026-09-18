use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};

use crate::utils::DATA_DIR;

const DB_FILENAME: &str = "app.db";
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS transcripts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
    duration_ms INTEGER NOT NULL,
    text TEXT NOT NULL,
    audio_path TEXT,
    model TEXT
);
CREATE INDEX IF NOT EXISTS idx_timestamp ON transcripts(timestamp DESC);
";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transcript {
    pub id: i64,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: i64,
    pub text: String,
    pub audio_path: String,
    pub model: String,
}

pub struct Db {
    conn: Connection,
    path: PathBuf,
}

impl Db {
    pub fn new() -> Result<Self> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(&*DATA_DIR)?;
        std::fs::set_permissions(&*DATA_DIR, std::fs::Permissions::from_mode(0o700))?;
        let path = DATA_DIR.join(DB_FILENAME);
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Self::open(&path)
    }

    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| anyhow!("failed to open database: {e}"))?;

        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;

        let db = Self {
            conn,
            path: path.to_path_buf(),
        };
        db.init()?;
        Ok(db)
    }

    fn init(&self) -> Result<()> {
        self.conn
            .execute_batch(SCHEMA)
            .map_err(|e| anyhow!("failed to create schema: {e}"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn save_transcript(
        &self,
        duration_ms: i64,
        text: &str,
        audio_path: &str,
        model: &str,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO transcripts (duration_ms, text, audio_path, model) VALUES (?, ?, ?, ?)",
                params![duration_ms, text, audio_path, model],
            )
            .map_err(|e| anyhow!("failed to save transcript: {e}"))?;
        Ok(())
    }

    pub fn get_last_transcript(&self) -> Result<Option<Transcript>> {
        Ok(self.get_transcripts(1)?.into_iter().next())
    }

    /// Returns the most recent transcripts. A non-positive `limit` returns all.
    pub fn get_transcripts(&self, limit: i64) -> Result<Vec<Transcript>> {
        let mut query = String::from(
            "SELECT id, timestamp, duration_ms, text, audio_path, model FROM transcripts ORDER BY timestamp DESC, id DESC",
        );
        if limit > 0 {
            query.push_str(" LIMIT ?");
        }

        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|e| anyhow!("failed to query transcripts: {e}"))?;

        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<Transcript> {
            Ok(Transcript {
                id: row.get(0)?,
                timestamp: parse_timestamp(&row.get::<_, String>(1)?)?,
                duration_ms: row.get(2)?,
                text: row.get(3)?,
                audio_path: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                model: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            })
        };

        let rows = if limit > 0 {
            stmt.query_map(params![limit], map_row)
        } else {
            stmt.query_map([], map_row)
        }
        .map_err(|e| anyhow!("failed to query transcripts: {e}"))?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| anyhow!("failed to scan transcript: {e}"))
    }
}

/// SQLite stores `CURRENT_TIMESTAMP` as `YYYY-MM-DD HH:MM:SS` in UTC.
fn parse_timestamp(raw: &str) -> rusqlite::Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(raw, fmt) {
            return Ok(naive.and_utc());
        }
    }
    Err(rusqlite::Error::FromSqlConversionFailure(
        1,
        rusqlite::types::Type::Text,
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid transcript timestamp: {raw:?}"),
        )
        .into(),
    ))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn round_trips_transcripts() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();

        assert!(db.get_last_transcript().unwrap().is_none());

        db.save_transcript(1500, "hello world", "/tmp/a.wav", "whisper-1")
            .unwrap();
        db.save_transcript(2500, "second", "/tmp/b.wav", "whisper-1")
            .unwrap();
        db.conn
            .execute(
                "UPDATE transcripts SET timestamp = ?",
                params!["2026-01-01 00:00:00"],
            )
            .unwrap();

        let all = db.get_transcripts(-1).unwrap();
        assert_eq!(all.len(), 2);

        let one = db.get_transcripts(1).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].text, "second");
        assert_eq!(one[0].model, "whisper-1");

        let last = db.get_last_transcript().unwrap().unwrap();
        assert_eq!(last.id, one[0].id);
    }

    #[test]
    fn rejects_malformed_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();
        db.conn
            .execute(
                "INSERT INTO transcripts (timestamp, duration_ms, text) VALUES (?, ?, ?)",
                params!["not-a-timestamp", 1, "bad row"],
            )
            .unwrap();

        let err = db.get_transcripts(1).unwrap_err().to_string();
        assert!(err.contains("failed to scan transcript"));
    }

    #[test]
    #[ignore = "subprocess probe"]
    fn private_database_probe_child() {
        let db = Db::new().unwrap();
        assert_eq!(
            std::fs::metadata(&*DATA_DIR).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(db.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn creates_private_database() {
        let dir = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::tests::private_database_probe_child",
                "--ignored",
            ])
            .env("XDG_DATA_HOME", dir.path().join("data"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
