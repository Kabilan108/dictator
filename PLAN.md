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
│   ├── transcribe/
│   │   └── whisper.go       # Whisper API client
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
    - Max Duration: 300 seconds (enforce in recorder, provide flag in the `dictator start` command)

**Implementation Notes:**
```go
// Use malgo (miniaudio Go bindings) for cross-platform audio
// Alternative: portaudio-go
import "github.com/gen2brain/malgo"

type Recorder struct {
    device     *malgo.Device
    context    *malgo.AllocatedContext
    buffers    [][]byte
    recording  bool
}

// Key methods:
// - Start() error
// - Stop() []byte  // Returns PCM data
// - IsRecording() bool
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

#### 4.3 Whisper API Client (`internal/transcribe/whisper.go`)

**Requirements:**
- Send audio to OpenAI Whisper API
- Handle API authentication
- Return transcribed text

**Implementation Notes:**
```go
// Use standard net/http with multipart form upload
type WhisperClient struct {
    apiKey   string
    endpoint string
}

// API endpoint: https://api.openai.com/v1/audio/transcriptions
// Method: POST with multipart/form-data
// Fields: file (audio), model ("whisper-1"), language ("en")
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

**Config Structure:**
```json
{
  "api": {
    "endpoint": "https://api.openai.com/v1/audio/transcriptions",
    "key": "api-key-string-xxxxx",
    "model": "whisper-1"
  },
  "audio": {
    "sample_rate": 16000,
    "channels": 1
  },
  "behavior": {
    "typing_delay_ms": 10,
    "max_recording_seconds": 300
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

### 7. CLI Commands

- The CLI should be implemented using cobra to set up commands and viper to handle configuration

```bash
dictator daemon   # Run the daemon (foreground)
dictator start    # Start recording
dictator stop     # Stop recording and transcribe
dictator toggle   # Toggle recording (start if idle, stop if recording)
dictator cancel   # Cancel current operation
dictator status   # Get current state (returns: idle|recording|transcribing|typing)
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
github.com/godbus/dbus/v5      // D-Bus for notifications
github.com/gen2brain/malgo     // Audio recording
```

### 11. Build and Packaging

**Nix Derivation:**
```nix
{ lib, buildGoModule, fetchFromGitHub, xdotool }:

buildGoModule rec {
  pname = "dictator";
  version = "0.1.0";

  src = ./.;

  vendorSha256 = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

  buildInputs = [ xdotool ];

  postInstall = ''
    # Install systemd service file
    install -Dm644 dictator.service $out/share/systemd/user/dictator.service
  '';
}
```
