use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use chrono::{
    DateTime, Days, Duration as ChronoDuration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc,
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize};

use crate::utils::DATA_DIR;

const DB_FILENAME: &str = "app.db";
const SCHEMA_VERSION: i64 = 1;
const MAX_HISTORY_PAGE: usize = 100;
const RECENT_LATENCY_LIMIT: i64 = 60;
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
CREATE TABLE IF NOT EXISTS failed_transcriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    duration_ms INTEGER NOT NULL,
    audio_path TEXT NOT NULL UNIQUE
);
CREATE INDEX IF NOT EXISTS idx_failed_transcriptions_timestamp
    ON failed_transcriptions(timestamp DESC);
CREATE TABLE IF NOT EXISTS recordings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    duration_ms INTEGER NOT NULL,
    text TEXT NOT NULL DEFAULT '',
    audio_path TEXT,
    model TEXT,
    status TEXT NOT NULL CHECK (status IN ('complete', 'failed')),
    error TEXT,
    current_revision INTEGER NOT NULL DEFAULT 0,
    legacy_transcript_id INTEGER UNIQUE,
    legacy_failed_id INTEGER UNIQUE
);
CREATE INDEX IF NOT EXISTS idx_recordings_timestamp_id
    ON recordings(timestamp DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_recordings_status_timestamp_id
    ON recordings(status, timestamp DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_recordings_audio_path ON recordings(audio_path);
CREATE TABLE IF NOT EXISTS transcript_revisions (
    recording_id INTEGER NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    revision_no INTEGER NOT NULL,
    text TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    source TEXT NOT NULL,
    PRIMARY KEY (recording_id, revision_no)
);
CREATE TABLE IF NOT EXISTS transcription_attempts (
    recording_id INTEGER NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    attempt_no INTEGER NOT NULL,
    started_at DATETIME NOT NULL,
    finished_at DATETIME NOT NULL,
    latency_ms INTEGER,
    status TEXT NOT NULL CHECK (status IN ('complete', 'error')),
    error TEXT,
    model TEXT,
    PRIMARY KEY (recording_id, attempt_no)
);
CREATE INDEX IF NOT EXISTS idx_attempts_finished
    ON transcription_attempts(finished_at DESC, recording_id DESC, attempt_no DESC);
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FailedTranscription {
    pub id: i64,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: i64,
    pub audio_path: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RecordingStatus {
    #[default]
    Complete,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum HistoryFilter {
    #[default]
    All,
    Edited,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryCursor {
    pub timestamp: DateTime<Utc>,
    pub id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryQuery {
    pub search: Option<String>,
    pub filter: HistoryFilter,
    pub limit: usize,
    pub before: Option<HistoryCursor>,
}

impl Default for HistoryQuery {
    fn default() -> Self {
        Self {
            search: None,
            filter: HistoryFilter::All,
            limit: 50,
            before: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordingSummary {
    pub id: i64,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: i64,
    pub text: String,
    pub audio_path: Option<String>,
    pub model: Option<String>,
    pub status: RecordingStatus,
    pub error: Option<String>,
    pub current_revision: i64,
    pub attempt_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptRevision {
    pub recording_id: i64,
    pub revision_no: i64,
    pub text: String,
    pub created_at: DateTime<Utc>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptionAttempt {
    pub recording_id: i64,
    pub attempt_no: i64,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub latency_ms: Option<i64>,
    pub status: String,
    pub error: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordingDetail {
    pub recording: RecordingSummary,
    pub revisions: Vec<TranscriptRevision>,
    pub attempts: Vec<TranscriptionAttempt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryPage {
    pub recordings: Vec<RecordingSummary>,
    pub next_cursor: Option<HistoryCursor>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DailyRecordingCount {
    pub date: NaiveDate,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRecordingStats {
    pub model: String,
    pub recordings: i64,
    pub duration_ms: i64,
    pub latency_p95_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LatencyStats {
    pub sample_count: usize,
    pub recent_ms: Vec<i64>,
    pub p50_ms: Option<i64>,
    pub p95_ms: Option<i64>,
    pub p99_ms: Option<i64>,
    pub max_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordingStats {
    pub total_recordings: i64,
    pub successful_recordings: i64,
    pub failed_recordings: i64,
    pub edited_recordings: i64,
    pub total_words: i64,
    pub total_duration_ms: i64,
    pub today_words: i64,
    pub today_duration_ms: i64,
    pub daily_last_7: Vec<DailyRecordingCount>,
    pub by_model: Vec<ModelRecordingStats>,
    pub latency: LatencyStats,
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
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let mut db = Self {
            conn,
            path: path.to_path_buf(),
        };
        db.init()?;
        Ok(db)
    }

    fn init(&mut self) -> Result<()> {
        let transaction = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| anyhow!("failed to start database migration: {e}"))?;
        let version: i64 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            bail!(
                "database schema version {version} is newer than supported version {SCHEMA_VERSION}"
            );
        }
        transaction.execute_batch(SCHEMA)?;
        backfill_recordings(&transaction)?;
        if version < SCHEMA_VERSION {
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        transaction
            .commit()
            .map_err(|e| anyhow!("failed to migrate database: {e}"))
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
        self.save_transcript_attempt(duration_ms, text, audio_path, model, None)
            .map(|_| ())
    }

    pub fn save_transcript_attempt(
        &self,
        duration_ms: i64,
        text: &str,
        audio_path: &str,
        model: &str,
        latency_ms: Option<i64>,
    ) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO transcripts (duration_ms, text, audio_path, model) VALUES (?, ?, ?, ?)",
            params![duration_ms, text, audio_path, model],
        )?;
        let legacy_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO recordings
             (duration_ms, text, audio_path, model, status, current_revision, legacy_transcript_id)
             VALUES (?, ?, ?, ?, 'complete', 0, ?)",
            params![duration_ms, text, audio_path, model, legacy_id],
        )?;
        let recording_id = tx.last_insert_rowid();
        insert_revision(&tx, recording_id, 0, text, "model")?;
        insert_attempt(&tx, recording_id, latency_ms, "complete", None, Some(model))?;
        tx.commit()?;
        Ok(recording_id)
    }

    pub fn save_failed_transcription(&self, duration_ms: i64, audio_path: &str) -> Result<()> {
        self.save_failed_attempt(
            duration_ms,
            Some(audio_path),
            "transcription failed",
            None,
            None,
        )
        .map(|_| ())
    }

    /// Saves a terminal failure. `audio_path` may be absent when capture failed
    /// before the daemon could create a WAV file.
    pub fn save_failed_attempt(
        &self,
        duration_ms: i64,
        audio_path: Option<&str>,
        error: &str,
        model: Option<&str>,
        latency_ms: Option<i64>,
    ) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        let legacy_id = if let Some(path) = audio_path {
            tx.execute(
                "INSERT OR REPLACE INTO failed_transcriptions (duration_ms, audio_path) VALUES (?, ?)",
                params![duration_ms, path],
            )?;
            Some(tx.last_insert_rowid())
        } else {
            None
        };
        let existing = if let Some(path) = audio_path {
            tx.query_row(
                "SELECT id FROM recordings WHERE audio_path = ? AND status = 'failed'
                 ORDER BY timestamp DESC, id DESC LIMIT 1",
                params![path],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        } else {
            None
        };
        let recording_id = if let Some(id) = existing {
            tx.execute(
                "UPDATE recordings SET duration_ms = ?, error = ?, model = COALESCE(?, model),
                 legacy_failed_id = ? WHERE id = ?",
                params![duration_ms, error, model, legacy_id, id],
            )?;
            id
        } else {
            tx.execute(
                "INSERT INTO recordings
                 (duration_ms, audio_path, model, status, error, current_revision, legacy_failed_id)
                 VALUES (?, ?, ?, 'failed', ?, 0, ?)",
                params![duration_ms, audio_path, model, error, legacy_id],
            )?;
            tx.last_insert_rowid()
        };
        insert_attempt(&tx, recording_id, latency_ms, "error", Some(error), model)?;
        tx.commit()?;
        Ok(recording_id)
    }

    pub fn get_last_failed_transcription(&self) -> Result<Option<FailedTranscription>> {
        self.sync_legacy_writes()?;
        self.conn
            .query_row(
                "SELECT id, timestamp, duration_ms, audio_path FROM recordings
                 WHERE status = 'failed' AND audio_path IS NOT NULL
                 ORDER BY COALESCE(legacy_failed_id, id) DESC LIMIT 1",
                [],
                |row| {
                    Ok(FailedTranscription {
                        id: row.get(0)?,
                        timestamp: parse_timestamp(&row.get::<_, String>(1)?)?,
                        duration_ms: row.get(2)?,
                        audio_path: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|e| anyhow!("failed to get last failed transcription: {e}"))
    }

    pub fn save_retried_transcript(
        &self,
        duration_ms: i64,
        text: &str,
        audio_path: &str,
        model: &str,
    ) -> Result<()> {
        self.save_retried_transcript_attempt(duration_ms, text, audio_path, model, None)
            .map(|_| ())
    }

    pub fn save_retried_transcript_attempt(
        &self,
        duration_ms: i64,
        text: &str,
        audio_path: &str,
        model: &str,
        latency_ms: Option<i64>,
    ) -> Result<i64> {
        self.complete_retry_attempt(None, duration_ms, text, audio_path, model, latency_ms)
    }

    /// Completes the exact failed recording selected by the GUI/IPC caller.
    pub fn save_retried_recording_attempt(
        &self,
        recording_id: i64,
        duration_ms: i64,
        text: &str,
        audio_path: &str,
        model: &str,
        latency_ms: Option<i64>,
    ) -> Result<i64> {
        self.complete_retry_attempt(
            Some(recording_id),
            duration_ms,
            text,
            audio_path,
            model,
            latency_ms,
        )
    }

    fn complete_retry_attempt(
        &self,
        requested_recording_id: Option<i64>,
        duration_ms: i64,
        text: &str,
        audio_path: &str,
        model: &str,
        latency_ms: Option<i64>,
    ) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        // Preserve the legacy insert/delete behavior for CLI callers and old
        // databases. The canonical recording is updated in the same transaction.
        tx.execute(
            "INSERT INTO transcripts (duration_ms, text, audio_path, model) VALUES (?, ?, ?, ?)",
            params![duration_ms, text, audio_path, model],
        )?;
        let legacy_id = tx.last_insert_rowid();
        let existing = if let Some(id) = requested_recording_id {
            tx.query_row(
                "SELECT id FROM recordings WHERE id = ? AND status = 'failed' AND audio_path = ?",
                params![id, audio_path],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        } else {
            tx.query_row(
                "SELECT id FROM recordings WHERE audio_path = ? AND status = 'failed'
                 ORDER BY timestamp DESC, id DESC LIMIT 1",
                params![audio_path],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        };
        if requested_recording_id.is_some() && existing.is_none() {
            bail!("selected recording is no longer a failed recording with this audio");
        }
        let recording_id = if let Some(id) = existing {
            tx.execute(
                "UPDATE recordings SET duration_ms = ?, text = ?, model = ?, status = 'complete',
                 error = NULL, current_revision = 0, legacy_transcript_id = ? WHERE id = ?",
                params![duration_ms, text, model, legacy_id, id],
            )?;
            id
        } else {
            tx.execute(
                "INSERT INTO recordings
                 (duration_ms, text, audio_path, model, status, current_revision, legacy_transcript_id)
                 VALUES (?, ?, ?, ?, 'complete', 0, ?)",
                params![duration_ms, text, audio_path, model, legacy_id],
            )?;
            tx.last_insert_rowid()
        };
        tx.execute(
            "INSERT OR IGNORE INTO transcript_revisions
             (recording_id, revision_no, text, source) VALUES (?, 0, ?, 'model')",
            params![recording_id, text],
        )?;
        insert_attempt(&tx, recording_id, latency_ms, "complete", None, Some(model))?;
        tx.execute(
            "DELETE FROM failed_transcriptions WHERE audio_path = ?",
            params![audio_path],
        )?;
        tx.commit()?;
        Ok(recording_id)
    }

    pub fn record_retry_failure(
        &self,
        recording_id: i64,
        error: &str,
        model: &str,
        latency_ms: Option<i64>,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        if tx.execute(
            "UPDATE recordings SET status = 'failed', error = ?, model = ? WHERE id = ?",
            params![error, model, recording_id],
        )? == 0
        {
            bail!("recording {recording_id} does not exist");
        }
        insert_attempt(
            &tx,
            recording_id,
            latency_ms,
            "error",
            Some(error),
            Some(model),
        )?;
        tx.commit()?;
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

    pub fn get_recording(&self, recording_id: i64) -> Result<Option<RecordingDetail>> {
        self.sync_legacy_writes()?;
        let Some(recording) = self.recording_summary(recording_id)? else {
            return Ok(None);
        };
        let mut revisions = self.conn.prepare(
            "SELECT recording_id, revision_no, text, created_at, source
             FROM transcript_revisions WHERE recording_id = ? ORDER BY revision_no",
        )?;
        let revisions = revisions
            .query_map(params![recording_id], map_revision)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut attempts = self.conn.prepare(
            "SELECT recording_id, attempt_no, started_at, finished_at, latency_ms, status, error, model
             FROM transcription_attempts WHERE recording_id = ? ORDER BY attempt_no",
        )?;
        let attempts = attempts
            .query_map(params![recording_id], map_attempt)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(RecordingDetail {
            recording,
            revisions,
            attempts,
        }))
    }

    pub fn get_history(&self, query: &HistoryQuery) -> Result<HistoryPage> {
        self.sync_legacy_writes()?;
        let limit = query.limit.clamp(1, MAX_HISTORY_PAGE);
        let search = query.search.as_deref().unwrap_or("").trim().to_lowercase();
        let filter = match query.filter {
            HistoryFilter::All => "all",
            HistoryFilter::Edited => "edited",
            HistoryFilter::Failed => "failed",
        };
        let (cursor_timestamp, cursor_id) = query.before.as_ref().map_or((None, None), |cursor| {
            (Some(sql_timestamp(cursor.timestamp)), Some(cursor.id))
        });
        let predicate = "(?1 = '' OR lower(text) LIKE '%' || ?1 || '%'
            OR lower(COALESCE(error, '')) LIKE '%' || ?1 || '%')
            AND (?2 = 'all' OR (?2 = 'edited' AND current_revision > 0)
            OR (?2 = 'failed' AND status = 'failed'))";
        let total = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM recordings WHERE {predicate}"),
            params![search, filter],
            |row| row.get(0),
        )?;
        let sql = format!(
            "SELECT id, timestamp, duration_ms, text, audio_path, model, status, error,
                    current_revision,
                    (SELECT COUNT(*) FROM transcription_attempts a WHERE a.recording_id = recordings.id)
             FROM recordings WHERE {predicate}
               AND (?3 IS NULL OR timestamp < ?3 OR (timestamp = ?3 AND id < ?4))
             ORDER BY timestamp DESC, id DESC LIMIT ?5"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut recordings = stmt
            .query_map(
                params![
                    search,
                    filter,
                    cursor_timestamp,
                    cursor_id,
                    limit as i64 + 1
                ],
                map_recording_summary,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let has_more = recordings.len() > limit;
        recordings.truncate(limit);
        let next_cursor = has_more.then(|| {
            let last = recordings
                .last()
                .expect("a page with more rows is nonempty");
            HistoryCursor {
                timestamp: last.timestamp,
                id: last.id,
            }
        });
        Ok(HistoryPage {
            recordings,
            next_cursor,
            total,
        })
    }

    pub fn append_revision(
        &self,
        recording_id: i64,
        expected_revision: i64,
        text: &str,
        source: &str,
    ) -> Result<TranscriptRevision> {
        let tx = self.conn.unchecked_transaction()?;
        let next = expected_revision
            .checked_add(1)
            .ok_or_else(|| anyhow!("revision overflow"))?;
        if tx.execute(
            "UPDATE recordings SET text = ?, current_revision = ?
             WHERE id = ? AND status = 'complete' AND current_revision = ?",
            params![text, next, recording_id, expected_revision],
        )? == 0
        {
            let actual = tx
                .query_row(
                    "SELECT current_revision FROM recordings WHERE id = ?",
                    params![recording_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            match actual {
                Some(actual) => bail!(
                    "revision conflict for recording {recording_id}: expected {expected_revision}, current {actual}"
                ),
                None => bail!("recording {recording_id} does not exist"),
            }
        }
        tx.execute(
            "UPDATE transcripts SET text = ?
             WHERE id = (SELECT legacy_transcript_id FROM recordings WHERE id = ?)",
            params![text, recording_id],
        )?;
        insert_revision(&tx, recording_id, next, text, source)?;
        let revision = tx.query_row(
            "SELECT recording_id, revision_no, text, created_at, source
             FROM transcript_revisions WHERE recording_id = ? AND revision_no = ?",
            params![recording_id, next],
            map_revision,
        )?;
        tx.commit()?;
        Ok(revision)
    }

    pub fn restore_revision(
        &self,
        recording_id: i64,
        revision_no: i64,
        expected_revision: i64,
    ) -> Result<TranscriptRevision> {
        let text = self
            .conn
            .query_row(
                "SELECT text FROM transcript_revisions WHERE recording_id = ? AND revision_no = ?",
                params![recording_id, revision_no],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| {
                anyhow!("revision {revision_no} does not exist for recording {recording_id}")
            })?;
        self.append_revision(recording_id, expected_revision, &text, "restore")
    }

    pub fn get_stats(&self) -> Result<RecordingStats> {
        self.sync_legacy_writes()?;
        let (
            total_recordings,
            successful_recordings,
            failed_recordings,
            edited_recordings,
            total_duration_ms,
        ) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(status = 'complete'), 0),
                        COALESCE(SUM(status = 'failed'), 0), COALESCE(SUM(current_revision > 0), 0),
                        COALESCE(SUM(CASE WHEN status = 'complete' THEN duration_ms ELSE 0 END), 0)
                 FROM recordings",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
        let texts = self
            .conn
            .prepare("SELECT text FROM recordings WHERE status = 'complete'")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let total_words = texts
            .iter()
            .map(|text| text.split_whitespace().count() as i64)
            .sum();
        let local_today = Local::now().date_naive();
        let local_midnight = Local
            .from_local_datetime(
                &local_today
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight is a valid local time"),
            )
            .earliest()
            .expect("the local day has a midnight");
        let today_start = sql_timestamp(local_midnight.with_timezone(&Utc));
        let today_rows = self
            .conn
            .prepare(
                "SELECT text, duration_ms FROM recordings
                 WHERE status = 'complete' AND timestamp >= ?",
            )?
            .query_map(params![today_start], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let today_words = today_rows
            .iter()
            .map(|(text, _)| text.split_whitespace().count() as i64)
            .sum();
        let today_duration_ms = today_rows.iter().map(|(_, duration)| duration).sum();
        let today = Local::now().date_naive();
        let start = today.checked_sub_days(Days::new(6)).unwrap_or(today);
        let mut daily_last_7 = (0..7)
            .filter_map(|offset| start.checked_add_days(Days::new(offset)))
            .map(|date| DailyRecordingCount { date, count: 0 })
            .collect::<Vec<_>>();
        let mut days = self.conn.prepare(
            "SELECT date(timestamp, 'localtime'), COUNT(*) FROM recordings
             WHERE status = 'complete' AND date(timestamp, 'localtime') >= ?
             GROUP BY date(timestamp, 'localtime')",
        )?;
        for row in days.query_map(params![start.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })? {
            let (date, count) = row?;
            if let Ok(date) = NaiveDate::parse_from_str(&date, "%Y-%m-%d")
                && let Some(day) = daily_last_7.iter_mut().find(|day| day.date == date)
            {
                day.count = count;
            }
        }
        let mut models = self.conn.prepare(
            "SELECT COALESCE(NULLIF(model, ''), 'unknown'), COUNT(*), SUM(duration_ms)
             FROM recordings WHERE status = 'complete'
             GROUP BY COALESCE(NULLIF(model, ''), 'unknown') ORDER BY COUNT(*) DESC",
        )?;
        let mut by_model = models
            .query_map([], |row| {
                Ok(ModelRecordingStats {
                    model: row.get(0)?,
                    recordings: row.get(1)?,
                    duration_ms: row.get(2)?,
                    latency_p95_ms: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for model in &mut by_model {
            model.latency_p95_ms = percentile(&self.recent_latencies(Some(&model.model))?, 0.95);
        }
        let recent_ms = self.recent_latencies(None)?;
        let latency = LatencyStats {
            sample_count: recent_ms.len(),
            p50_ms: percentile(&recent_ms, 0.50),
            p95_ms: percentile(&recent_ms, 0.95),
            p99_ms: percentile(&recent_ms, 0.99),
            max_ms: recent_ms.iter().copied().max(),
            recent_ms,
        };
        Ok(RecordingStats {
            total_recordings,
            successful_recordings,
            failed_recordings,
            edited_recordings,
            total_words,
            total_duration_ms,
            today_words,
            today_duration_ms,
            daily_last_7,
            by_model,
            latency,
        })
    }

    fn recording_summary(&self, id: i64) -> Result<Option<RecordingSummary>> {
        self.conn.query_row(
            "SELECT id, timestamp, duration_ms, text, audio_path, model, status, error,
                    current_revision,
                    (SELECT COUNT(*) FROM transcription_attempts a WHERE a.recording_id = recordings.id)
             FROM recordings WHERE id = ?",
            params![id], map_recording_summary,
        ).optional().map_err(Into::into)
    }

    fn recent_latencies(&self, model: Option<&str>) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT latency_ms FROM transcription_attempts
             WHERE status = 'complete' AND latency_ms IS NOT NULL
               AND (?1 IS NULL OR COALESCE(NULLIF(model, ''), 'unknown') = ?1)
             ORDER BY finished_at DESC, recording_id DESC, attempt_no DESC LIMIT ?2",
        )?;
        Ok(stmt
            .query_map(params![model, RECENT_LATENCY_LIMIT], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Imports rows written by a pre-migration daemon during a rolling upgrade.
    fn sync_legacy_writes(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        backfill_recordings(&tx)?;
        tx.commit()?;
        Ok(())
    }
}

fn backfill_recordings(tx: &Transaction<'_>) -> Result<()> {
    // A pre-migration daemon uses INSERT OR REPLACE for repeated failures.
    // Relink that new legacy row to the existing canonical recording by path.
    let replaced_failures = {
        let mut stmt = tx.prepare(
            "SELECT f.id, f.duration_ms, f.audio_path
             FROM failed_transcriptions f
             WHERE NOT EXISTS (SELECT 1 FROM recordings r WHERE r.legacy_failed_id = f.id)",
        )?;
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (legacy_id, duration_ms, audio_path) in replaced_failures {
        tx.execute(
            "UPDATE recordings SET legacy_failed_id = ?, duration_ms = ?
             WHERE id = (SELECT id FROM recordings
                         WHERE status = 'failed' AND audio_path = ?
                         ORDER BY timestamp DESC, id DESC LIMIT 1)",
            params![legacy_id, duration_ms, audio_path],
        )?;
    }

    // An old daemon completes retry by inserting a transcript and deleting the
    // failed row. Match it by the retained audio path before generic backfill so
    // one capture remains one canonical recording.
    let legacy_retries = {
        let mut stmt = tx.prepare(
            "SELECT r.id, t.id, t.duration_ms, t.text, t.audio_path, t.model, t.timestamp
             FROM recordings r JOIN transcripts t ON t.audio_path = r.audio_path
             WHERE r.status = 'failed'
               AND NOT EXISTS (SELECT 1 FROM recordings done WHERE done.legacy_transcript_id = t.id)
               AND t.id = (SELECT MAX(candidate.id) FROM transcripts candidate
                           WHERE candidate.audio_path = r.audio_path)",
        )?;
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (recording_id, transcript_id, duration_ms, text, audio_path, model, timestamp) in
        legacy_retries
    {
        tx.execute(
            "UPDATE recordings SET duration_ms = ?, text = ?, audio_path = ?, model = ?,
             status = 'complete', error = NULL, current_revision = 0, legacy_transcript_id = ?
             WHERE id = ?",
            params![
                duration_ms,
                text,
                audio_path,
                model,
                transcript_id,
                recording_id
            ],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO transcript_revisions
             (recording_id, revision_no, text, created_at, source)
             VALUES (?, 0, ?, ?, 'model')",
            params![recording_id, text, timestamp],
        )?;
        tx.execute(
            "INSERT INTO transcription_attempts
             (recording_id, attempt_no, started_at, finished_at, status, model)
             VALUES (?, (SELECT COALESCE(MAX(attempt_no), 0) + 1
                         FROM transcription_attempts WHERE recording_id = ?),
                     ?, ?, 'complete', ?)",
            params![recording_id, recording_id, timestamp, timestamp, model],
        )?;
    }

    tx.execute(
        "INSERT INTO recordings
         (timestamp, duration_ms, text, audio_path, model, status, current_revision, legacy_transcript_id)
         SELECT t.timestamp, t.duration_ms, t.text, t.audio_path, t.model, 'complete', 0, t.id
         FROM transcripts t
         WHERE NOT EXISTS (
             SELECT 1 FROM recordings r WHERE r.legacy_transcript_id = t.id
         )",
        [],
    )?;
    tx.execute(
        "INSERT INTO recordings
         (timestamp, duration_ms, text, audio_path, status, error, current_revision, legacy_failed_id)
         SELECT f.timestamp, f.duration_ms, '', f.audio_path, 'failed', 'transcription failed', 0, f.id
         FROM failed_transcriptions f
         WHERE NOT EXISTS (
             SELECT 1 FROM recordings r WHERE r.legacy_failed_id = f.id
         )",
        [],
    )?;
    tx.execute(
        "INSERT INTO transcript_revisions (recording_id, revision_no, text, created_at, source)
         SELECT r.id, 0, r.text, r.timestamp, 'model' FROM recordings r
         WHERE r.status = 'complete'
           AND NOT EXISTS (
               SELECT 1 FROM transcript_revisions revisions
               WHERE revisions.recording_id = r.id AND revisions.revision_no = 0
           )",
        [],
    )?;
    tx.execute(
        "INSERT INTO transcription_attempts
         (recording_id, attempt_no, started_at, finished_at, status, error, model)
         SELECT r.id, 1, r.timestamp, r.timestamp,
                CASE r.status WHEN 'complete' THEN 'complete' ELSE 'error' END,
                r.error, r.model FROM recordings r
         WHERE NOT EXISTS (
             SELECT 1 FROM transcription_attempts attempts
             WHERE attempts.recording_id = r.id
         )",
        [],
    )?;
    Ok(())
}

fn insert_revision(
    tx: &Transaction<'_>,
    recording_id: i64,
    revision_no: i64,
    text: &str,
    source: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO transcript_revisions (recording_id, revision_no, text, source) VALUES (?, ?, ?, ?)",
        params![recording_id, revision_no, text, source],
    )?;
    Ok(())
}

fn insert_attempt(
    tx: &Transaction<'_>,
    recording_id: i64,
    latency_ms: Option<i64>,
    status: &str,
    error: Option<&str>,
    model: Option<&str>,
) -> Result<()> {
    let finished_at = Utc::now();
    let started_at = latency_ms
        .and_then(|millis| {
            finished_at.checked_sub_signed(ChronoDuration::milliseconds(millis.max(0)))
        })
        .unwrap_or(finished_at);
    tx.execute(
        "INSERT INTO transcription_attempts
         (recording_id, attempt_no, started_at, finished_at, latency_ms, status, error, model)
         VALUES (?, (SELECT COALESCE(MAX(attempt_no), 0) + 1 FROM transcription_attempts WHERE recording_id = ?),
                 ?, ?, ?, ?, ?, ?)",
        params![
            recording_id, recording_id, sql_timestamp(started_at), sql_timestamp(finished_at),
            latency_ms, status, error, model
        ],
    )?;
    Ok(())
}

fn map_recording_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordingSummary> {
    let status = match row.get::<_, String>(6)?.as_str() {
        "complete" => RecordingStatus::Complete,
        "failed" => RecordingStatus::Failed,
        other => {
            return Err(invalid_data(
                6,
                format!("invalid recording status: {other}"),
            ));
        }
    };
    Ok(RecordingSummary {
        id: row.get(0)?,
        timestamp: parse_timestamp(&row.get::<_, String>(1)?)?,
        duration_ms: row.get(2)?,
        text: row.get(3)?,
        audio_path: row.get(4)?,
        model: row.get(5)?,
        status,
        error: row.get(7)?,
        current_revision: row.get(8)?,
        attempt_count: row.get(9)?,
    })
}

fn map_revision(row: &rusqlite::Row<'_>) -> rusqlite::Result<TranscriptRevision> {
    Ok(TranscriptRevision {
        recording_id: row.get(0)?,
        revision_no: row.get(1)?,
        text: row.get(2)?,
        created_at: parse_timestamp(&row.get::<_, String>(3)?)?,
        source: row.get(4)?,
    })
}

fn map_attempt(row: &rusqlite::Row<'_>) -> rusqlite::Result<TranscriptionAttempt> {
    Ok(TranscriptionAttempt {
        recording_id: row.get(0)?,
        attempt_no: row.get(1)?,
        started_at: parse_timestamp(&row.get::<_, String>(2)?)?,
        finished_at: parse_timestamp(&row.get::<_, String>(3)?)?,
        latency_ms: row.get(4)?,
        status: row.get(5)?,
        error: row.get(6)?,
        model: row.get(7)?,
    })
}

fn percentile(values: &[i64], fraction: f64) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted
        .get(((sorted.len() - 1) as f64 * fraction).ceil() as usize)
        .copied()
}

fn sql_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y-%m-%d %H:%M:%S%.f").to_string()
}

fn invalid_data(column: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        std::io::Error::new(std::io::ErrorKind::InvalidData, message).into(),
    )
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
    Err(invalid_data(
        1,
        format!("invalid transcript timestamp: {raw:?}"),
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
    fn failed_transcriptions_persist_and_use_stable_latest_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        {
            let db = Db::open(&path).unwrap();
            assert!(db.get_last_failed_transcription().unwrap().is_none());
            db.save_failed_transcription(1_500, "/tmp/first.wav")
                .unwrap();
            db.save_failed_transcription(2_500, "/tmp/second.wav")
                .unwrap();
            db.conn
                .execute(
                    "UPDATE failed_transcriptions SET timestamp = ?",
                    params!["2026-01-01 00:00:00"],
                )
                .unwrap();
            let latest = db.get_last_failed_transcription().unwrap().unwrap();
            assert_eq!(latest.audio_path, "/tmp/second.wav");
        }

        let reopened = Db::open(&path).unwrap();
        let latest = reopened.get_last_failed_transcription().unwrap().unwrap();
        assert_eq!(latest.audio_path, "/tmp/second.wav");
        assert_eq!(latest.duration_ms, 2_500);
    }

    #[test]
    fn last_failed_lookup_imports_legacy_write_after_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        let db = Db::open(&path).unwrap();
        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute(
                "INSERT INTO failed_transcriptions (duration_ms, audio_path) VALUES (?, ?)",
                params![1_750, "/tmp/late-failure.wav"],
            )
            .unwrap();

        let failed = db.get_last_failed_transcription().unwrap().unwrap();
        assert_eq!(failed.duration_ms, 1_750);
        assert_eq!(failed.audio_path, "/tmp/late-failure.wav");
    }

    #[test]
    fn failed_transcription_upsert_keeps_one_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();
        db.save_failed_transcription(1_500, "/tmp/retry.wav")
            .unwrap();
        db.save_failed_transcription(3_000, "/tmp/retry.wav")
            .unwrap();

        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM failed_transcriptions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
        let failed = db.get_last_failed_transcription().unwrap().unwrap();
        assert_eq!(failed.duration_ms, 3_000);
    }

    #[test]
    fn resaving_failed_transcription_makes_it_latest() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();
        db.save_failed_transcription(1_500, "/tmp/first.wav")
            .unwrap();
        db.save_failed_transcription(2_500, "/tmp/second.wav")
            .unwrap();
        db.save_failed_transcription(3_000, "/tmp/first.wav")
            .unwrap();

        let latest = db.get_last_failed_transcription().unwrap().unwrap();
        assert_eq!(latest.audio_path, "/tmp/first.wav");
        assert_eq!(latest.duration_ms, 3_000);
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM failed_transcriptions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn retry_saves_transcript_and_removes_only_matching_failure() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();
        db.save_failed_transcription(1_500, "/tmp/retry.wav")
            .unwrap();
        db.save_failed_transcription(2_500, "/tmp/other.wav")
            .unwrap();

        db.save_retried_transcript(
            1_500,
            "recovered text",
            "/tmp/retry.wav",
            "gpt-4o-transcribe",
        )
        .unwrap();

        let transcripts = db.get_transcripts(-1).unwrap();
        assert_eq!(transcripts.len(), 1);
        assert_eq!(transcripts[0].text, "recovered text");
        assert_eq!(transcripts[0].audio_path, "/tmp/retry.wav");

        let remaining: Vec<String> = db
            .conn
            .prepare("SELECT audio_path FROM failed_transcriptions ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(remaining, vec!["/tmp/other.wav"]);
    }

    #[test]
    fn failed_retry_keeps_pending_failure() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();
        db.save_failed_transcription(1_500, "/tmp/retry.wav")
            .unwrap();
        db.conn
            .execute_batch(
                "CREATE TRIGGER reject_transcript
                 BEFORE INSERT ON transcripts
                 BEGIN
                     SELECT RAISE(ABORT, 'forced insert failure');
                 END;",
            )
            .unwrap();

        assert!(
            db.save_retried_transcript(
                1_500,
                "recovered text",
                "/tmp/retry.wav",
                "gpt-4o-transcribe",
            )
            .is_err()
        );
        assert!(db.get_transcripts(-1).unwrap().is_empty());
        let pending = db.get_last_failed_transcription().unwrap().unwrap();
        assert_eq!(pending.audio_path, "/tmp/retry.wav");
    }

    #[test]
    fn migration_is_idempotent_and_backfills_revision_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE transcripts (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
                 duration_ms INTEGER NOT NULL, text TEXT NOT NULL,
                 audio_path TEXT, model TEXT
             );
             CREATE TABLE failed_transcriptions (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 timestamp DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 duration_ms INTEGER NOT NULL,
                 audio_path TEXT NOT NULL UNIQUE
             );",
            )
            .unwrap();
        legacy
            .execute(
                "INSERT INTO transcripts (timestamp, duration_ms, text, audio_path, model)
             VALUES ('2026-01-01 01:02:03', 1000, 'old text', '/tmp/old.wav', 'old-model')",
                [],
            )
            .unwrap();
        legacy
            .execute(
                "INSERT INTO failed_transcriptions (timestamp, duration_ms, audio_path)
             VALUES ('2026-01-02 01:02:03', 2000, '/tmp/fail.wav')",
                [],
            )
            .unwrap();
        drop(legacy);

        for _ in 0..2 {
            let db = Db::open(&path).unwrap();
            let page = db.get_history(&HistoryQuery::default()).unwrap();
            assert_eq!(page.total, 2);
            let completed = page
                .recordings
                .iter()
                .find(|row| row.status == RecordingStatus::Complete)
                .unwrap();
            let detail = db.get_recording(completed.id).unwrap().unwrap();
            assert_eq!(detail.revisions.len(), 1);
            assert_eq!(detail.revisions[0].revision_no, 0);
            assert_eq!(detail.revisions[0].text, "old text");
            assert_eq!(detail.attempts.len(), 1);
        }
    }

    #[test]
    fn rejects_newer_schema_and_imports_legacy_writes_after_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        let db = Db::open(&path).unwrap();
        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute(
                "INSERT INTO transcripts (duration_ms, text, audio_path, model)
             VALUES (1000, 'written by old daemon', '/tmp/late.wav', 'old')",
                [],
            )
            .unwrap();
        assert_eq!(db.get_history(&HistoryQuery::default()).unwrap().total, 1);
        drop(db);
        legacy
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        drop(legacy);
        assert!(
            Db::open(&path)
                .err()
                .unwrap()
                .to_string()
                .contains("newer than supported")
        );
    }

    #[test]
    fn rolling_upgrade_reconciles_old_daemon_retry_without_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        let db = Db::open(&path).unwrap();
        db.save_failed_transcription(1000, "/tmp/rolling.wav")
            .unwrap();
        let failed = db.get_history(&HistoryQuery::default()).unwrap().recordings[0].clone();
        let captured_at = failed.timestamp;

        let legacy = Connection::open(&path).unwrap();
        let tx = legacy.unchecked_transaction().unwrap();
        tx.execute(
            "INSERT INTO transcripts (duration_ms, text, audio_path, model)
             VALUES (1000, 'old daemon recovered', '/tmp/rolling.wav', 'old')",
            [],
        )
        .unwrap();
        tx.execute(
            "DELETE FROM failed_transcriptions WHERE audio_path = '/tmp/rolling.wav'",
            [],
        )
        .unwrap();
        tx.commit().unwrap();
        drop(legacy);

        let page = db.get_history(&HistoryQuery::default()).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.recordings[0].id, failed.id);
        assert_eq!(page.recordings[0].timestamp, captured_at);
        assert_eq!(page.recordings[0].status, RecordingStatus::Complete);
        assert_eq!(page.recordings[0].text, "old daemon recovered");
        let detail = db.get_recording(failed.id).unwrap().unwrap();
        assert_eq!(detail.attempts.len(), 2);
    }

    #[test]
    fn repeated_read_reconciliation_does_not_consume_recording_ids() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();
        let first = db
            .save_transcript_attempt(1000, "first", "/tmp/first.wav", "m", Some(10))
            .unwrap();
        for _ in 0..20 {
            db.get_history(&HistoryQuery::default()).unwrap();
            db.get_stats().unwrap();
        }
        let second = db
            .save_transcript_attempt(1000, "second", "/tmp/second.wav", "m", Some(10))
            .unwrap();
        assert_eq!(second, first + 1);
    }

    #[test]
    fn retry_preserves_identity_timestamp_and_attempt_latencies() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();
        let id = db
            .save_failed_attempt(
                1000,
                Some("/tmp/fail.wav"),
                "timeout",
                Some("m1"),
                Some(2100),
            )
            .unwrap();
        let captured_at = db.get_recording(id).unwrap().unwrap().recording.timestamp;
        db.record_retry_failure(id, "still down", "m2", Some(900))
            .unwrap();
        let recovered = db
            .save_retried_transcript_attempt(1000, "recovered", "/tmp/fail.wav", "m2", Some(450))
            .unwrap();
        assert_eq!(recovered, id);
        let detail = db.get_recording(id).unwrap().unwrap();
        assert_eq!(detail.recording.timestamp, captured_at);
        assert_eq!(detail.recording.status, RecordingStatus::Complete);
        assert_eq!(
            detail
                .attempts
                .iter()
                .map(|attempt| attempt.latency_ms)
                .collect::<Vec<_>>(),
            vec![Some(2100), Some(900), Some(450)],
        );
        assert_eq!(detail.revisions[0].text, "recovered");
    }

    #[test]
    fn revisions_are_immutable_and_conflicts_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();
        let id = db
            .save_transcript_attempt(1000, "model text", "/tmp/a.wav", "m", Some(100))
            .unwrap();
        assert_eq!(
            db.append_revision(id, 0, "user edit", "user")
                .unwrap()
                .revision_no,
            1
        );
        assert!(
            db.append_revision(id, 0, "lost edit", "user")
                .unwrap_err()
                .to_string()
                .contains("revision conflict")
        );
        let restored = db.restore_revision(id, 0, 1).unwrap();
        assert_eq!(restored.revision_no, 2);
        assert_eq!(restored.text, "model text");
        let detail = db.get_recording(id).unwrap().unwrap();
        assert_eq!(detail.revisions[0].text, "model text");
        assert_eq!(detail.recording.current_revision, 2);
        assert_eq!(db.get_transcripts(1).unwrap()[0].text, "model text");
    }

    #[test]
    fn history_uses_bounded_keyset_pagination_and_filters() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();
        let first = db
            .save_transcript_attempt(1000, "alpha needle", "/tmp/a.wav", "m", Some(100))
            .unwrap();
        db.append_revision(first, 0, "alpha edited needle", "user")
            .unwrap();
        db.save_failed_attempt(0, None, "microphone unavailable", Some("m"), None)
            .unwrap();
        db.save_transcript_attempt(1000, "omega", "/tmp/b.wav", "m", Some(300))
            .unwrap();

        let page1 = db
            .get_history(&HistoryQuery {
                limit: 1,
                ..HistoryQuery::default()
            })
            .unwrap();
        assert_eq!(page1.recordings.len(), 1);
        let page2 = db
            .get_history(&HistoryQuery {
                limit: 1,
                before: page1.next_cursor.clone(),
                ..HistoryQuery::default()
            })
            .unwrap();
        assert_ne!(page1.recordings[0].id, page2.recordings[0].id);
        let edited = db
            .get_history(&HistoryQuery {
                filter: HistoryFilter::Edited,
                ..HistoryQuery::default()
            })
            .unwrap();
        assert_eq!(edited.recordings[0].id, first);
        let failed = db
            .get_history(&HistoryQuery {
                search: Some("microphone".into()),
                filter: HistoryFilter::Failed,
                ..HistoryQuery::default()
            })
            .unwrap();
        assert_eq!(failed.recordings.len(), 1);
        assert!(failed.recordings[0].audio_path.is_none());
    }

    #[test]
    fn stats_use_recent_successful_attempt_latencies() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();
        for latency in [100, 200, 300, 400] {
            db.save_transcript_attempt(
                60_000,
                "two words",
                &format!("/tmp/{latency}.wav"),
                "m",
                Some(latency),
            )
            .unwrap();
        }
        db.save_failed_attempt(0, None, "failed", Some("m"), Some(9999))
            .unwrap();
        let stats = db.get_stats().unwrap();
        assert_eq!(stats.total_recordings, 5);
        assert_eq!(stats.total_words, 8);
        assert_eq!(stats.today_words, 8);
        assert_eq!(stats.today_duration_ms, 240_000);
        assert_eq!(stats.latency.sample_count, 4);
        assert_eq!(stats.latency.p50_ms, Some(300));
        assert_eq!(stats.latency.p95_ms, Some(400));
        assert_eq!(stats.by_model[0].latency_p95_ms, Some(400));
        assert_eq!(stats.daily_last_7.len(), 7);
        assert_eq!(
            stats.daily_last_7.last().unwrap().date,
            Local::now().date_naive()
        );
        assert_eq!(stats.daily_last_7.last().unwrap().count, 4);
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
