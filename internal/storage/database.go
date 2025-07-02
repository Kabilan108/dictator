package storage

import (
	"database/sql"
	"fmt"
	"os"
	"path/filepath"

	"github.com/kabilan108/dictator/internal/utils"
	_ "github.com/mattn/go-sqlite3"
)

const (
	dbFilename = "app.db"
	schema     = `
CREATE TABLE IF NOT EXISTS transcripts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
    duration_ms INTEGER NOT NULL,
    text TEXT NOT NULL,
    audio_path TEXT,
    model TEXT
);
CREATE INDEX IF NOT EXISTS idx_timestamp ON transcripts(timestamp DESC);
`
)

type DB struct {
	conn *sql.DB
	path string
}

func NewDB() (*DB, error) {
	if err := os.MkdirAll(utils.CACHE_DIR, 0o755); err != nil {
		return nil, err
	}

	dbPath := filepath.Join(utils.CACHE_DIR, dbFilename)

	conn, err := sql.Open("sqlite3", dbPath+"?_journal_mode=WAL&_busy_timeout=5000")
	if err != nil {
		return nil, fmt.Errorf("failed to open database: %w", err)
	}

	db := &DB{
		conn: conn,
		path: dbPath,
	}

	if err := db.init(); err != nil {
		conn.Close()
		return nil, err
	}

	return db, nil
}

func (db *DB) init() error {
	if _, err := db.conn.Exec(schema); err != nil {
		return fmt.Errorf("failed to create schema: %w", err)
	}
	return nil
}

func (db *DB) Close() error {
	if db.conn != nil {
		return db.conn.Close()
	}
	return nil
}

func (db *DB) Path() string {
	return db.path
}
