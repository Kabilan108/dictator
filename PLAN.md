# Dictator - Voice-to-Text Input System

## Project Overview

Dictator is a voice-to-text daemon for Linux that enables voice typing anywhere the cursor is positioned. The system consists of a background daemon that handles audio recording and transcription, and a CLI tool for controlling the daemon. Visual feedback is provided through dunst notifications.

### Key Features
- Global voice input that types at cursor position
- Visual feedback via dunst notifications
- Whisper API integration for transcription
- Unix socket IPC between CLI and daemon
- i3 window manager integration via keybindings

### System Architecture

```
┌─────────────┐     ┌──────────────┐     ┌──────────────┐
│   i3 WM     │────►│  dictator    │────►│  dictator    │
│ (hotkeys)   │ exec│   (CLI)      │ IPC │  (daemon)    │
└─────────────┘     └──────────────┘     └───────┬──────┘
                                                 │
                    ┌──────────────┐             │
                    │    dunst     │◄────────────┤
                    │(notifications)│    D-Bus   │
                    └──────────────┘             │
                                                 │
                    ┌──────────────┐             │
                    │   xdotool    │◄────────────┤
                    │  (typing)    │             │
                    └──────────────┘             │
                                                 │
                    ┌──────────────┐             │
                    │ OpenAI API   │◄────────────┘
                    │  (Whisper)   │    HTTPS
                    └──────────────┘
```

## Implementation Plan

### 1. Project Structure

```
dictator/
├── internal/
│   ├── audio/
│   │   └── recorder.go      # Audio recording logic
│   ├── cmd/
│   │   └── commands.go      # CLI command handlers
│   ├── config/
│   │   └── config.go        # Config parsing
│   ├── daemon/
│   │   └── daemon.go        # daemon implementation
│   ├── notifier/
│   │   └── dunst.go         # Dunst integration
│   ├── utils/
│   │   └── utils.go         # Utilities, logger, and file management
│   └── typing/
│       └── xdotool.go       # Keyboard input
│   └── ipc/
│       ├── client.go        # IPC client
│       ├── protocol.go      # IPC protocol definitions
│       └── server.go        # IPC server
├── go.mod
├── go.sum
├── main.go
├── default.nix              # Nix package
└── dictator.service         # Systemd service file
```

### 2. Single Binary Architecture

The `dictator` binary will support two modes:

- Daemon mode: `dictator daemon` - runs the background service
- Client mode: `dictator [command]` - sends commands to the daemon

### 3. Daemon Lifecycle

#### Running the daemon

1. For testing/development:

```
dictator daemon     # foreground
dictator daemon &   # background
```

2. With systemd

```
systemctl --user start dictator
systemctl --user enable dictator
```

3. In tmux

```
tmux new -d -s dictator 'dictator daemon'
```

#### systemd service file

```
[Unit]
Description=Dictator Voice Input Daemon
After=graphical-session.target

[Service]
Type=simple
ExecStart=%h/.nix-profile/bin/dictator daemon
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=default.target
```

### 4. Core Components

#### 4.1 Audio Recording (`internal/audio/recorder.go`)

**Requirements:**
- Record from default microphone
- Support start/stop operations
- Return PCM audio data suitable for Whisper API
    - Sample Rate: 16kHz (required by Whisper)
    - Channels: Mono (1 channel)
    - Format: 16-bit PCM
    - Container: WAV format for API upload
    - Max Duration: configurable via audio config (enforced in recorder)

**Implementation Notes:**
```go
// Uses PortAudio Go bindings for cross-platform audio
import "github.com/gordonklaus/portaudio"

type Recorder struct {
    stream        *portaudio.Stream
    buffer        []float32
    isInitialized bool
    config        config.AudioConfig
    mu            sync.RWMutex
    state         RecorderState
    audioData     []byte
    startTime     time.Time
    doneChan      chan struct{}
    errorChan     chan error
    durationTimer *time.Timer
    wg            sync.WaitGroup
    log           utils.Logger
}

// Key methods:
// - NewRecorder(config.AudioConfig, utils.LogLevel) (*Recorder, error)
// - Start() error
// - Stop() ([]byte, string, error)  // Returns WAV data, file path, error
// - Close() error
// - IsRecording() bool
// - GetRecordingDuration() time.Duration
// - EncodeToWAV([]byte) ([]byte, error)
// - HasTimedOut() bool

// State management:
// - StateIdle, StateRecording, StateStopped
// - Thread-safe with mutex protection
// - Automatic timeout handling with configurable duration
```

#### 4.2 Dunst Notifier (`internal/notifier/dunst.go`)

**Requirements:**
- Create/update/dismiss notifications
- Use consistent notification ID for smooth updates
- Show state transitions: Recording → Transcribing → Typing

**Implementation Notes:**
```go
// Use godbus/dbus for D-Bus communication
import "github.com/godbus/dbus/v5"

type DunstNotifier struct {
    conn           *dbus.Conn
    notificationID uint32
}

// Key methods:
// - Update(title, body, icon string) error
// - Close() error
```

**D-Bus Interface:**
- Service `org.freedesktop.Notifications`
- ObjectPath `/org/freedesktop/Notifications`
- Method: `Notify` for updates
- Method: `CloseNotification` for dismissal

#### 4.3 Whisper API Client (`internal/audio/transcribe.go`)

**Requirements:**
- Send audio to Whisper API (OpenAI-compatible)
- Handle API authentication
- Return transcribed text
- Support retry logic for reliability

**Implementation Notes:**
```go
// Uses standard net/http with multipart form upload
type WhisperClient interface {
    Transcribe(ctx context.Context, req *TranscriptionRequest) (*TranscriptionResponse, error)
}

type whisperClient struct {
    config     *config.APIConfig
    httpClient *http.Client
    log        utils.Logger
}

type TranscriptionRequest struct {
    AudioData []byte
    Filename  string
    Model     string // optional, defaults to config or "distil-large-v3"
    Language  string // optional
}

type TranscriptionResponse struct {
    Text string `json:"text"`
}

// Key features:
// - Configurable endpoint with automatic path completion
// - Bearer token authentication
// - 2-attempt retry logic with 1-second delay
// - Comprehensive error handling and logging
// - Model fallback chain (request > config > "distil-large-v3")
// - Timeout support via context
```

**Endpoint Schema:**
Here is the `openapi.json` file for the whisper API we are using
```
{"openapi":"3.1.0","info":{"title":"siren","description":"API for transcribing audio using Whisper, compatible with OpenAI schema","version":"1.0.0"},"paths":{"/v1/models":{"get":{"summary":"List Models","description":"List available models in OpenAI-compatible format","operationId":"list_models_v1_models_get","responses":{"200":{"description":"Successful Response","content":{"application/json":{"schema":{"$ref":"#/components/schemas/ModelsResponse"}}}}},"security":[{"HTTPBearer":[]}]}},"/v1/audio/transcriptions":{"post":{"summary":"Transcribe Audio","description":"OpenAI compatible transcription endpoint","operationId":"transcribe_audio_v1_audio_transcriptions_post","requestBody":{"content":{"multipart/form-data":{"schema":{"$ref":"#/components/schemas/Body_transcribe_audio_v1_audio_transcriptions_post"}}},"required":true},"responses":{"200":{"description":"Successful Response","content":{"application/json":{"schema":{"$ref":"#/components/schemas/TranscriptionResponse"}}}},"422":{"description":"Validation Error","content":{"application/json":{"schema":{"$ref":"#/components/schemas/HTTPValidationError"}}}}},"security":[{"HTTPBearer":[]}]}}},"components":{"schemas":{"Body_transcribe_audio_v1_audio_transcriptions_post":{"properties":{"file":{"type":"string","format":"binary","title":"File"},"model":{"anyOf":[{"type":"string"},{"type":"null"}],"title":"Model","description":"ID of the model to use. Only whisper-1 (which is powered by our open source Whisper V2 model) is currently available."},"language":{"anyOf":[{"type":"string"},{"type":"null"}],"title":"Language","description":"The language of the input audio. Supplying the input language in ISO-639-1 format will improve accuracy and latency."}},"type":"object","required":["file"],"title":"Body_transcribe_audio_v1_audio_transcriptions_post"},"HTTPValidationError":{"properties":{"detail":{"items":{"$ref":"#/components/schemas/ValidationError"},"type":"array","title":"Detail"}},"type":"object","title":"HTTPValidationError"},"ModelInfo":{"properties":{"id":{"type":"string","title":"Id"}},"type":"object","required":["id"],"title":"ModelInfo"},"ModelsResponse":{"properties":{"data":{"items":{"$ref":"#/components/schemas/ModelInfo"},"type":"array","title":"Data"}},"type":"object","required":["data"],"title":"ModelsResponse"},"TranscriptionResponse":{"properties":{"text":{"type":"string","title":"Text"}},"type":"object","required":["text"],"title":"TranscriptionResponse"},"ValidationError":{"properties":{"loc":{"items":{"anyOf":[{"type":"string"},{"type":"integer"}]},"type":"array","title":"Location"},"msg":{"type":"string","title":"Message"},"type":{"type":"string","title":"Error Type"}},"type":"object","required":["loc","msg","type"],"title":"ValidationError"}},"securitySchemes":{"HTTPBearer":{"type":"http","scheme":"bearer"}}}}
```

#### 4.4 Keyboard Input (`internal/typing/xdotool.go`)

**Requirements:**
- Type text at current cursor position
- Work across all applications
- Handle special characters properly

**Implementation Notes:**
```go
// Use os/exec to call xdotool
import "os/exec"

func TypeText(text string) error {
    // Use xdotool type with proper escaping
    cmd := exec.Command("xdotool", "type", "--clearmodifiers", "--", text)
    return cmd.Run()
}

// Note: --clearmodifiers ensures modifiers don't interfere
// The -- prevents text from being interpreted as options
```

#### 4.5 IPC Protocol (`internal/ipc/`)

**Protocol Design:**
```go
type Command struct {
    ID     string   `json:"id"`     // For request correlation
    Action string   `json:"action"`
    Args   []string `json:"args,omitempty"`
    Timestamp int64 `json:"timestamp"`
}

type Response struct {
    ID      string            `json:"id"`      // Matches request ID
    Success bool              `json:"success"`
    Error   string            `json:"error,omitempty"`
    Data    map[string]string `json:"data,omitempty"`
}
```

**Socket Path:** `/tmp/dictator.sock`

#### 4.6 Configuration (`internal/config/config.go`)

**Config File Location:** `~/.config/dictator/config.json`

**Implementation Notes:**
```go
type Config struct {
    API   APIConfig   `json:"api" mapstructure:"api"`
    App   AppConfig   `json:"app" mapstructure:"app"`
    Audio AudioConfig `json:"audio" mapstructure:"audio"`
}

type APIConfig struct {
    Endpoint   string `json:"endpoint" mapstructure:"endpoint"`
    Key        string `json:"key" mapstructure:"key"`
    Model      string `json:"model" mapstructure:"model"`
    TimeoutSec int    `json:"timeout" mapstructure:"timeout"`
}

type AudioConfig struct {
    SampleRate     int `json:"sample_rate" mapstructure:"sample_rate"`
    Channels       int `json:"channels" mapstructure:"channels"`
    BitDepth       int `json:"bit_depth" mapstructure:"bit_depth"`
    FramesPerBlock int `json:"frames_per_block" mapstructure:"frames_per_block"`
    MaxDurationMin int `json:"max_duration_min" mapstructure:"max_duration_min"`
}

type AppConfig struct {
    LogLevel        utils.LogLevel `json:"log_level" mapstructure:"log_level"`
    MaxRecordingMin int           `json:"max_recording_min" mapstructure:"max_recording_seconds"`
    TypingDelayMS   int           `json:"typing_delay_ms" mapstructure:"typing_delay_ms"`
}

// Key features:
// - Uses Viper for configuration management with environment variable support
// - Automatic config file creation with sensible defaults
// - Comprehensive validation of all config fields
// - Global config singleton pattern
// - Integrates with utils package for directory management
```

**Default Config Structure:**
```json
{
  "api": {
    "endpoint": "https://sietch.sole-pierce.ts.net/siren/v1/audio/transcriptions",
    "key": "",
    "model": "distil-large-v3",
    "timeout": 60
  },
  "audio": {
    "sample_rate": 16000,
    "channels": 1,
    "bit_depth": 16,
    "frames_per_block": 1024,
    "max_duration_min": 5
  },
  "app": {
    "log_level": 0,
    "max_recording_min": 5,
    "typing_delay_ms": 10
  }
}
```

### 5. Daemon Implementation

The daemon runs a simple event loop with signal handling:

```
// internal/daemon/daemon.go
func (d *Daemon) Run() error {
    // Start IPC server
    if err := d.ipcServer.Start(); err != nil {
        return fmt.Errorf("failed to start IPC server: %w", err)
    }
    defer d.ipcServer.Stop()

    // Setup signal handling
    sigChan := make(chan os.Signal, 1)
    signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)

    log.Println("Dictator daemon started")

    // Main event loop
    for {
        select {
        case sig := <-sigChan:
            log.Printf("Received signal %v, shutting down", sig)
            return d.shutdown()

        case <-d.stopChan:
            log.Println("Daemon stop requested")
            return d.shutdown()
        }
    }
}
```

### 6. Daemon State Machine

```
States:
- Idle: Waiting for commands
- Recording: Actively recording audio
- Transcribing: Sending to Whisper API
- Typing: Typing out result

Transitions:
- Idle → Recording (on "start" command)
- Recording → Transcribing (on "stop" command)
- Transcribing → Typing (on successful API response)
- Typing → Idle (after typing completes)
- Any → Idle (on "cancel" command)
```

- Note: use mutex/waitgroups to track state and manage concurrency where necessary and appropriate

### 7. CLI Commands (`internal/cmd/commands.go`)

**Implementation Notes:**
```go
// Uses Cobra for CLI framework with Viper for configuration
// Current implementation includes basic command structure

// Commands implemented:
// - daemon: Runs full audio recording and transcription test (10 seconds)
// - start, stop, toggle, cancel, status: Command stubs for future IPC integration

// The daemon command currently:
// 1. Initializes configuration
// 2. Creates recorder with PortAudio
// 3. Creates Whisper client
// 4. Records for 10 seconds
// 5. Stops recording and saves WAV file
// 6. Transcribes audio via API
// 7. Logs transcript result
```

```bash
dictator daemon   # Run the daemon (currently: test recording/transcription)
dictator start    # Start recording (stub - needs IPC integration)
dictator stop     # Stop recording and transcribe (stub - needs IPC integration)
dictator toggle   # Toggle recording (stub - needs IPC integration)
dictator cancel   # Cancel current operation (stub - needs IPC integration)
dictator status   # Get current state (stub - needs IPC integration)
```

### 8. i3 Integration

```i3config
# In ~/.config/i3/config
bindsym $mod+Shift+v exec --no-startup-id dictator toggle
bindsym $mod+Shift+c exec --no-startup-id dictator cancel
```

### 9. Error Handling Strategy

**Optimistic Flow:** Always attempt to complete the user's intent:

1. **Network Errors:** If API fails, show error notification but don't crash
2. **Audio Errors:** If recording fails, notify and return to idle
3. **Typing Errors:** If xdotool fails, copy text to clipboard as fallback
4. **Daemon Connection:** If daemon not running, suggest starting it
5. **Graceful Degradation:** Each stage should handle failures and notify user

**Error Recovery Flows**

1. **Recording Fails**: Show notification, check microphone permissions
2. **API Timeout**: Retry once, then save audio file for manual retry
3. **Typing Fails**: Copy to clipboard, show notification with instructions

### 10. External Dependencies

**System Packages Required:**
- `xdotool` - For typing text at cursor position
- `xclip` - For clipboard fallback when `xdotool` is not available
- `pulseaudio` or `pipewire` - Audio system (already present in NixOS config)
- `dunst` - Notification daemon (already present in user's config)

**Go Dependencies:**
```go
github.com/gordonklaus/portaudio  // Audio recording via PortAudio
github.com/spf13/cobra           // CLI framework
github.com/spf13/viper           // Configuration management
github.com/fatih/color           // Terminal colors for logging
```

**Additional Utilities Package:**
```go
// internal/utils/utils.go provides:
// - Structured logging with levels (Debug, Info, Warn, Error, Fatal)
// - Directory management for config and cache
// - File path utilities for recordings
// - Cross-platform config/cache directory detection
```

### 11. Build and Packaging

**Current Nix Support:**
```nix
# flake.nix includes PortAudio and pkg-config for development
buildInputs = [
  go
  pkg-config
  portaudio
  # ... other dependencies
];
```

**Future Nix Derivation:**
```nix
{ lib, buildGoModule, fetchFromGitHub, xdotool, portaudio, pkg-config }:

buildGoModule rec {
  pname = "dictator";
  version = "0.1.0";

  src = ./.;

  vendorSha256 = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ xdotool portaudio ];

  postInstall = ''
    # Install systemd service file
    install -Dm644 dictator.service $out/share/systemd/user/dictator.service
  '';
}
```

## Current Implementation Status

### Completed Components:
- ✅ **Audio Recording**: Full PortAudio integration with WAV encoding
- ✅ **Transcription**: OpenAI-compatible Whisper API client with retry logic
- ✅ **Configuration**: Viper-based config with validation and defaults
- ✅ **Utilities**: Logging, directory management, file paths
- ✅ **Basic CLI**: Cobra framework with working daemon test command

### Next Steps:
- 🔄 **IPC System**: Unix socket communication between CLI and daemon
- 🔄 **Daemon State Machine**: Proper state management and event loop
- 🔄 **Typing Integration**: xdotool integration for text input
- 🔄 **Notifications**: Dunst integration for visual feedback
- 🔄 **Error Handling**: Graceful error recovery and user feedback
