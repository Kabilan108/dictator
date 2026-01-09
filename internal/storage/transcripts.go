package storage

import (
	"database/sql"
	"fmt"
	"time"
)

type Transcript struct {
	ID         int       `json:"id"`
	Timestamp  time.Time `json:"timestamp"`
	DurationMs int       `json:"duration_ms"`
	Text       string    `json:"text"`
	AudioPath  string    `json:"audio_path"`
	Model      string    `json:"model"`
}

func (db *DB) SaveTranscript(durationMs int, text, audioPath, model string) error {
	query := `INSERT INTO transcripts (duration_ms, text, audio_path, model) VALUES (?, ?, ?, ?)`

	if _, err := db.conn.Exec(query, durationMs, text, audioPath, model); err != nil {
		return fmt.Errorf("failed to save transcript: %w", err)
	}

	return nil
}

func (db *DB) GetLastTranscript() (*Transcript, error) {
	query := `
SELECT id, timestamp, duration_ms, text, audio_path, model
FROM transcripts
ORDER BY timestamp DESC
LIMIT 1
`

	row := db.conn.QueryRow(query)

	var t Transcript
	err := row.Scan(&t.ID, &t.Timestamp, &t.DurationMs, &t.Text, &t.AudioPath, &t.Model)
	if err != nil {
		if err == sql.ErrNoRows {
			return nil, nil
		}
		return nil, fmt.Errorf("failed to get last transcript: %w", err)
	}

	return &t, nil
}

func (db *DB) GetTranscripts(limit int) ([]Transcript, error) {
	query := `
SELECT id, timestamp, duration_ms, text, audio_path, model
FROM transcripts
ORDER BY timestamp DESC
`
	var args []any

	if limit > 0 {
		query += " LIMIT ?"
		args = append(args, limit)
	}

	rows, err := db.conn.Query(query, args...)
	if err != nil {
		return nil, fmt.Errorf("failed to query transcripts: %w", err)
	}
	defer rows.Close()

	var transcripts []Transcript
	for rows.Next() {
		var t Transcript
		err := rows.Scan(&t.ID, &t.Timestamp, &t.DurationMs, &t.Text, &t.AudioPath, &t.Model)
		if err != nil {
			return nil, fmt.Errorf("failed to scan transcript: %w", err)
		}
		transcripts = append(transcripts, t)
	}

	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("error iterating transcripts: %w", err)
	}

	return transcripts, nil
}
