package ipc

import (
	"time"
)

// Command represents a request from cli to daemon
type Command struct {
	ID        string    `json:"id"`             // unique identifier for request correlation
	Action    string    `json:"action"`         // command action: start, stop, toggle, cancel, status
	Args      []string  `json:"args,omitempty"` // optional command arguments
	Timestamp time.Time `json:"timestamp"`      // request timestamp
}

// Response represents daemon's reply to cli command
type Response struct {
	ID      string            `json:"id"`              // matches request id
	Success bool              `json:"success"`         // whether command succeeded
	Error   string            `json:"error,omitempty"` // error message if failed
	Data    map[string]string `json:"data,omitempty"`  // additional response data
}

type DaemonState int

const (
	StateIdle DaemonState = iota
	StateRecording
	StateTranscribing
	StateTyping
	StateError
	StateStreaming
)

func (s DaemonState) String() string {
	switch s {
	case StateIdle:
		return "idle"
	case StateRecording:
		return "recording"
	case StateTranscribing:
		return "transcribing"
	case StateTyping:
		return "typing"
	case StateError:
		return "error"
	case StateStreaming:
		return "streaming"
	default:
		return "unknown"
	}
}

// StatusData represents the data returned by status command
type StatusData struct {
	State             DaemonState    `json:"state"`
	RecordingDuration *time.Duration `json:"recording_duration,omitempty"`
	LastError         *string        `json:"last_error,omitempty"`
	Uptime            time.Duration  `json:"uptime"`
}

// CommandActions define the available CLI commands
const (
	ActionStart  = "start"
	ActionStop   = "stop"
	ActionToggle = "toggle"
	ActionCancel = "cancel"
	ActionStatus = "status"
	ActionStream = "stream"
)

// Socket configuration
const (
	SocketPath = "/tmp/dictator.sock"
)

// Response data keys
const (
	DataKeyState             = "state"
	DataKeyRecordingDuration = "recording_duration"
	DataKeyLastError         = "last_error"
	DataKeyUptime            = "uptime"
	DataKeyText              = "text"
)

// Error messages
const (
	ErrInvalidCommand      = "invalid command"
	ErrAlreadyRecording    = "already recording"
	ErrNotRecording        = "not currently recording"
	ErrRecordingFailed     = "recording failed"
	ErrTranscriptionFailed = "transcription failed"
	ErrTypingFailed        = "typing failed"
)
