# Dictator Streaming Implementation Specification

This document specifies the implementation of real-time streaming transcription support for Dictator. This spec is self-contained and should be followed step-by-step.

## Overview

### Current State
- Voice typing daemon with batch transcription (record → stop → transcribe → type)
- X11 typing via `xdotool`, clipboard fallback via `xclip`
- HTTP POST to Whisper-compatible API
- State machine: idle → recording → transcribing → typing → idle

### Target State
- Real-time streaming transcription via WebSocket
- Wayland typing via `ydotool`
- Optional GTK layer-shell overlay for preview mode
- User-selectable modes: `streaming` (new) or `batch` (existing)
- State machine: idle → streaming (simultaneous record + transcribe + type) → idle

## Technical Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Protocol | WebSocket | Match Siren's streaming endpoint |
| Wayland typing | ydotool | Kernel-level uinput, compositor-agnostic |
| Overlay | Python + GTK4 + layer-shell | Mature ecosystem, easy Nix packaging |
| Overlay IPC | Unix socket | Simple, works well with Go |
| Typing strategy | Stable prefix | Only type finalized characters |

## Dependencies

### Go Dependencies
Add to `go.mod`:
```
github.com/gorilla/websocket v1.5.3
```

### System Dependencies
- `ydotool` + `ydotoold` for Wayland typing
- Python 3 + PyGObject + gtk4-layer-shell for overlay (optional)

---

## Part 1: Core Streaming Infrastructure

### Step 1.1: Add Configuration for Streaming

**File**: `internal/utils/config.go`

Add streaming configuration to the config structure:

```go
// Add after existing config types

// StreamingConfig holds streaming-specific settings
type StreamingConfig struct {
    Endpoint    string `json:"endpoint" mapstructure:"endpoint"`
    ChunkFrames int    `json:"chunk_frames" mapstructure:"chunk_frames"`
    Output      string `json:"output" mapstructure:"output"` // "direct" or "overlay"
}

// Update Config struct to include streaming
type Config struct {
    API       APIConfig       `json:"api" mapstructure:"api"`
    App       AppConfig       `json:"app" mapstructure:"app"`
    Audio     AudioConfig     `json:"audio" mapstructure:"audio"`
    Mode      string          `json:"mode" mapstructure:"mode"`           // "streaming" or "batch"
    Streaming StreamingConfig `json:"streaming" mapstructure:"streaming"`
}
```

Update `GetDefaultConfig()` to include streaming defaults:

```go
func GetDefaultConfig() *Config {
    return &Config{
        // ... existing defaults ...
        Mode: "batch", // Default to batch for backwards compatibility
        Streaming: StreamingConfig{
            Endpoint:    "ws://localhost:8000/ws/transcribe",
            ChunkFrames: 7,
            Output:      "direct",
        },
    }
}
```

Add validation for streaming config:

```go
func (c *Config) Validate() error {
    // ... existing validation ...

    // Validate mode
    if c.Mode != "streaming" && c.Mode != "batch" {
        return fmt.Errorf("mode must be 'streaming' or 'batch', got: %s", c.Mode)
    }

    // Validate streaming config
    if c.Mode == "streaming" {
        if c.Streaming.Endpoint == "" {
            return fmt.Errorf("streaming endpoint is required")
        }
        if c.Streaming.ChunkFrames < 1 || c.Streaming.ChunkFrames > 20 {
            return fmt.Errorf("chunk_frames must be between 1 and 20")
        }
        if c.Streaming.Output != "direct" && c.Streaming.Output != "overlay" {
            return fmt.Errorf("streaming output must be 'direct' or 'overlay'")
        }
    }

    return nil
}
```

**Verification**:
```bash
go build ./...
go vet ./...
```

---

### Step 1.2: Add ydotool Typing Backend

**File**: `internal/typing/typer.go`

Add ydotool support alongside existing xdotool:

```go
// YdotoolTyper implements Typer using ydotool for Wayland
type YdotoolTyper struct {
    typingDelay time.Duration
}

func NewYdotoolTyper(delayMS int) *YdotoolTyper {
    return &YdotoolTyper{
        typingDelay: time.Duration(delayMS) * time.Millisecond,
    }
}

func (t *YdotoolTyper) TypeText(ctx context.Context, text string) error {
    if text == "" {
        return nil
    }

    // ydotool type command
    cmd := exec.CommandContext(ctx, "ydotool", "type", "--", text)
    if err := cmd.Run(); err != nil {
        if ctx.Err() != nil {
            return ctx.Err()
        }
        return fmt.Errorf("ydotool type failed: %w", err)
    }

    if t.typingDelay > 0 {
        select {
        case <-time.After(t.typingDelay):
        case <-ctx.Done():
            return ctx.Err()
        }
    }

    return nil
}

func (t *YdotoolTyper) IsAvailable() bool {
    _, err := exec.LookPath("ydotool")
    return err == nil
}

// TypeIncremental types only new characters (for streaming)
func (t *YdotoolTyper) TypeIncremental(ctx context.Context, newChars string) error {
    return t.TypeText(ctx, newChars)
}

// Backspace sends backspace keys (for corrections)
func (t *YdotoolTyper) Backspace(ctx context.Context, count int) error {
    if count <= 0 {
        return nil
    }

    // ydotool key command for backspace
    // BackSpace key code
    for i := 0; i < count; i++ {
        cmd := exec.CommandContext(ctx, "ydotool", "key", "14:1", "14:0") // 14 = backspace
        if err := cmd.Run(); err != nil {
            if ctx.Err() != nil {
                return ctx.Err()
            }
            return fmt.Errorf("ydotool backspace failed: %w", err)
        }
    }

    return nil
}
```

Update `New()` to prefer ydotool on Wayland:

```go
func New(delayMS int) (Typer, error) {
    // Check for Wayland session
    waylandDisplay := os.Getenv("WAYLAND_DISPLAY")

    if waylandDisplay != "" {
        // Wayland: prefer ydotool
        ydotool := NewYdotoolTyper(delayMS)
        if ydotool.IsAvailable() {
            return ydotool, nil
        }
        // Fall through to X11 options
    }

    // X11 or fallback: prefer xdotool
    xdotool := NewXdotoolTyper(delayMS)
    if xdotool.IsAvailable() {
        return xdotool, nil
    }

    // Last resort: clipboard
    xclip := NewXclipTyper()
    if xclip.IsAvailable() {
        return xclip, nil
    }

    return nil, fmt.Errorf("no typing backend available (tried ydotool, xdotool, xclip)")
}
```

Add interface methods for streaming:

```go
// Typer defines the interface for typing text
type Typer interface {
    TypeText(ctx context.Context, text string) error
    IsAvailable() bool
}

// StreamingTyper extends Typer with incremental typing support
type StreamingTyper interface {
    Typer
    TypeIncremental(ctx context.Context, newChars string) error
    Backspace(ctx context.Context, count int) error
}
```

Also add these methods to `XdotoolTyper`:

```go
func (t *XdotoolTyper) TypeIncremental(ctx context.Context, newChars string) error {
    return t.TypeText(ctx, newChars)
}

func (t *XdotoolTyper) Backspace(ctx context.Context, count int) error {
    if count <= 0 {
        return nil
    }

    // xdotool key for backspace
    keys := make([]string, count)
    for i := range keys {
        keys[i] = "BackSpace"
    }

    cmd := exec.CommandContext(ctx, "xdotool", append([]string{"key", "--clearmodifiers"}, keys...)...)
    if err := cmd.Run(); err != nil {
        if ctx.Err() != nil {
            return ctx.Err()
        }
        return fmt.Errorf("xdotool backspace failed: %w", err)
    }

    return nil
}
```

**Verification**:
```bash
go build ./...
go vet ./...
# Manual test (requires ydotoold running):
# echo "test" | ydotool type --file -
```

---

### Step 1.3: Create WebSocket Streaming Client

**File**: `internal/streaming/client.go` (new file)

```go
package streaming

import (
    "context"
    "encoding/base64"
    "encoding/json"
    "fmt"
    "net/url"
    "sync"
    "time"

    "github.com/gorilla/websocket"
)

// Message types
const (
    MsgTypeConfig  = "config"
    MsgTypeAudio   = "audio"
    MsgTypeEnd     = "end"
    MsgTypePartial = "partial"
    MsgTypeFinal   = "final"
    MsgTypeError   = "error"
)

// ClientMessage is sent from client to server
type ClientMessage struct {
    Type        string `json:"type"`
    Data        string `json:"data,omitempty"`        // base64 audio for "audio" type
    Seq         int    `json:"seq,omitempty"`         // sequence number
    ChunkFrames int    `json:"chunk_frames,omitempty"` // for "config" type
}

// ServerMessage is received from server
type ServerMessage struct {
    Type      string `json:"type"`
    Text      string `json:"text,omitempty"`
    StableLen int    `json:"stable_len,omitempty"`
    Seq       int    `json:"seq,omitempty"`
    Message   string `json:"message,omitempty"` // for "error" type
    Code      string `json:"code,omitempty"`    // for "error" type
}

// PartialHandler is called for each partial transcription
type PartialHandler func(text string, stableLen int, seq int)

// FinalHandler is called for the final transcription
type FinalHandler func(text string)

// ErrorHandler is called on errors
type ErrorHandler func(err error)

// Client manages a WebSocket connection to the streaming server
type Client struct {
    endpoint   string
    apiKey     string
    chunkFrames int

    conn   *websocket.Conn
    connMu sync.Mutex
    seq    int

    onPartial PartialHandler
    onFinal   FinalHandler
    onError   ErrorHandler

    ctx    context.Context
    cancel context.CancelFunc
    done   chan struct{}
}

// NewClient creates a new streaming client
func NewClient(endpoint, apiKey string, chunkFrames int) *Client {
    return &Client{
        endpoint:    endpoint,
        apiKey:      apiKey,
        chunkFrames: chunkFrames,
        done:        make(chan struct{}),
    }
}

// SetHandlers sets the callback handlers
func (c *Client) SetHandlers(onPartial PartialHandler, onFinal FinalHandler, onError ErrorHandler) {
    c.onPartial = onPartial
    c.onFinal = onFinal
    c.onError = onError
}

// Connect establishes the WebSocket connection
func (c *Client) Connect(ctx context.Context) error {
    c.ctx, c.cancel = context.WithCancel(ctx)

    // Parse endpoint and add token
    u, err := url.Parse(c.endpoint)
    if err != nil {
        return fmt.Errorf("invalid endpoint: %w", err)
    }

    q := u.Query()
    q.Set("token", c.apiKey)
    u.RawQuery = q.Encode()

    // Connect
    dialer := websocket.Dialer{
        HandshakeTimeout: 10 * time.Second,
    }

    conn, _, err := dialer.DialContext(c.ctx, u.String(), nil)
    if err != nil {
        return fmt.Errorf("websocket connect failed: %w", err)
    }

    c.connMu.Lock()
    c.conn = conn
    c.connMu.Unlock()

    // Send config
    configMsg := ClientMessage{
        Type:        MsgTypeConfig,
        ChunkFrames: c.chunkFrames,
    }
    if err := c.sendMessage(configMsg); err != nil {
        conn.Close()
        return fmt.Errorf("failed to send config: %w", err)
    }

    // Start receiver goroutine
    go c.receiveLoop()

    return nil
}

// SendAudio sends an audio chunk to the server
func (c *Client) SendAudio(pcmData []byte) error {
    c.seq++
    msg := ClientMessage{
        Type: MsgTypeAudio,
        Data: base64.StdEncoding.EncodeToString(pcmData),
        Seq:  c.seq,
    }
    return c.sendMessage(msg)
}

// End signals end of audio stream
func (c *Client) End() error {
    msg := ClientMessage{Type: MsgTypeEnd}
    return c.sendMessage(msg)
}

// Close closes the connection
func (c *Client) Close() error {
    if c.cancel != nil {
        c.cancel()
    }

    c.connMu.Lock()
    defer c.connMu.Unlock()

    if c.conn != nil {
        err := c.conn.Close()
        c.conn = nil
        return err
    }
    return nil
}

// Wait waits for the connection to close
func (c *Client) Wait() {
    <-c.done
}

func (c *Client) sendMessage(msg ClientMessage) error {
    c.connMu.Lock()
    defer c.connMu.Unlock()

    if c.conn == nil {
        return fmt.Errorf("not connected")
    }

    data, err := json.Marshal(msg)
    if err != nil {
        return err
    }

    return c.conn.WriteMessage(websocket.TextMessage, data)
}

func (c *Client) receiveLoop() {
    defer close(c.done)

    for {
        select {
        case <-c.ctx.Done():
            return
        default:
        }

        c.connMu.Lock()
        conn := c.conn
        c.connMu.Unlock()

        if conn == nil {
            return
        }

        _, data, err := conn.ReadMessage()
        if err != nil {
            if c.ctx.Err() != nil {
                return // Context cancelled, expected
            }
            if c.onError != nil {
                c.onError(fmt.Errorf("read error: %w", err))
            }
            return
        }

        var msg ServerMessage
        if err := json.Unmarshal(data, &msg); err != nil {
            if c.onError != nil {
                c.onError(fmt.Errorf("json decode error: %w", err))
            }
            continue
        }

        switch msg.Type {
        case MsgTypePartial:
            if c.onPartial != nil {
                c.onPartial(msg.Text, msg.StableLen, msg.Seq)
            }
        case MsgTypeFinal:
            if c.onFinal != nil {
                c.onFinal(msg.Text)
            }
            return // Done after final
        case MsgTypeError:
            if c.onError != nil {
                c.onError(fmt.Errorf("server error [%s]: %s", msg.Code, msg.Message))
            }
            return
        }
    }
}
```

**Verification**:
```bash
go build ./...
go vet ./...
```

---

### Step 1.4: Create Streaming Handler

**File**: `internal/streaming/handler.go` (new file)

This manages the streaming session and coordinates audio → server → typing.

```go
package streaming

import (
    "context"
    "sync"

    "dictator/internal/typing"
)

// Handler manages a streaming transcription session
type Handler struct {
    client *Client
    typer  typing.StreamingTyper

    mu         sync.Mutex
    typedLen   int    // Characters already typed
    lastText   string // Last received text

    onStateChange func(state string)
}

// NewHandler creates a new streaming handler
func NewHandler(client *Client, typer typing.StreamingTyper) *Handler {
    return &Handler{
        client: client,
        typer:  typer,
    }
}

// SetStateCallback sets the state change callback
func (h *Handler) SetStateCallback(cb func(state string)) {
    h.onStateChange = cb
}

// Start begins a streaming session
func (h *Handler) Start(ctx context.Context) error {
    // Set up handlers
    h.client.SetHandlers(
        h.handlePartial,
        h.handleFinal,
        h.handleError,
    )

    // Connect
    if err := h.client.Connect(ctx); err != nil {
        return err
    }

    if h.onStateChange != nil {
        h.onStateChange("streaming")
    }

    return nil
}

// SendAudio sends audio data to the stream
func (h *Handler) SendAudio(pcmData []byte) error {
    return h.client.SendAudio(pcmData)
}

// Stop ends the streaming session
func (h *Handler) Stop(ctx context.Context) (string, error) {
    if err := h.client.End(); err != nil {
        return "", err
    }

    // Wait for final response
    h.client.Wait()

    h.mu.Lock()
    finalText := h.lastText
    h.mu.Unlock()

    if h.onStateChange != nil {
        h.onStateChange("idle")
    }

    return finalText, nil
}

// Cancel aborts the streaming session
func (h *Handler) Cancel() {
    h.client.Close()
    if h.onStateChange != nil {
        h.onStateChange("idle")
    }
}

func (h *Handler) handlePartial(text string, stableLen int, seq int) {
    h.mu.Lock()
    defer h.mu.Unlock()

    h.lastText = text

    // Type only newly stable characters
    if stableLen > h.typedLen {
        newText := text[h.typedLen:stableLen]
        if err := h.typer.TypeIncremental(context.Background(), newText); err != nil {
            // Log error but continue
        }
        h.typedLen = stableLen
    }
}

func (h *Handler) handleFinal(text string) {
    h.mu.Lock()
    defer h.mu.Unlock()

    h.lastText = text

    // Type any remaining characters not yet typed
    if len(text) > h.typedLen {
        remaining := text[h.typedLen:]
        if err := h.typer.TypeIncremental(context.Background(), remaining); err != nil {
            // Log error but continue
        }
        h.typedLen = len(text)
    }
}

func (h *Handler) handleError(err error) {
    // TODO: Propagate error to daemon
    if h.onStateChange != nil {
        h.onStateChange("error")
    }
}
```

**Verification**:
```bash
go build ./...
go vet ./...
```

---

## Part 2: Daemon Integration

### Step 2.1: Add Streaming State

**File**: `internal/ipc/protocol.go`

Add new state constant:

```go
const (
    StateIdle         = 0
    StateRecording    = 1
    StateTranscribing = 2
    StateTyping       = 3
    StateError        = 4
    StateStreaming    = 5 // New: simultaneous record + transcribe + type
)
```

Add new action:

```go
const (
    ActionStart  = "start"
    ActionStop   = "stop"
    ActionToggle = "toggle"
    ActionCancel = "cancel"
    ActionStatus = "status"
    ActionStream = "stream" // New: start streaming mode
)
```

**Verification**:
```bash
go build ./...
go vet ./...
```

---

### Step 2.2: Update Daemon for Streaming

**File**: `internal/daemon/daemon.go`

Add streaming handler field and integration:

```go
import (
    // ... existing imports ...
    "dictator/internal/streaming"
)

type Daemon struct {
    // ... existing fields ...
    streamHandler *streaming.Handler
}
```

Add streaming start handler:

```go
func (d *Daemon) HandleStream(ctx context.Context) error {
    d.mu.Lock()
    if d.state != StateIdle {
        d.mu.Unlock()
        return fmt.Errorf("cannot start streaming: not idle (state=%d)", d.state)
    }

    // Get streaming config
    cfg := d.config.Streaming
    provider := d.config.API.Providers[d.config.API.ActiveProvider]

    d.mu.Unlock()

    // Create streaming client
    client := streaming.NewClient(cfg.Endpoint, provider.Key, cfg.ChunkFrames)

    // Get typer (must support streaming interface)
    typer, ok := d.typer.(typing.StreamingTyper)
    if !ok {
        return fmt.Errorf("typer does not support streaming")
    }

    // Create handler
    handler := streaming.NewHandler(client, typer)
    handler.SetStateCallback(func(state string) {
        // Update daemon state based on streaming state
        switch state {
        case "streaming":
            d.setState(StateStreaming)
        case "idle":
            d.setState(StateIdle)
        case "error":
            d.handleError(fmt.Errorf("streaming error"))
        }
    })

    // Store handler
    d.mu.Lock()
    d.streamHandler = handler
    d.mu.Unlock()

    // Start streaming
    if err := handler.Start(ctx); err != nil {
        return err
    }

    // Start audio capture with streaming callback
    if err := d.recorder.StartStreaming(ctx, func(pcmData []byte) {
        handler.SendAudio(pcmData)
    }); err != nil {
        handler.Cancel()
        return err
    }

    return nil
}
```

Add streaming stop:

```go
func (d *Daemon) HandleStreamStop(ctx context.Context) (string, error) {
    d.mu.Lock()
    handler := d.streamHandler
    d.mu.Unlock()

    if handler == nil {
        return "", fmt.Errorf("no active streaming session")
    }

    // Stop recording
    d.recorder.Stop()

    // End stream and get final text
    text, err := handler.Stop(ctx)
    if err != nil {
        return "", err
    }

    d.mu.Lock()
    d.streamHandler = nil
    d.mu.Unlock()

    return text, nil
}
```

Update toggle handler to respect mode:

```go
func (d *Daemon) HandleToggle(ctx context.Context) error {
    d.mu.Lock()
    state := d.state
    mode := d.config.Mode
    d.mu.Unlock()

    switch state {
    case StateIdle:
        if mode == "streaming" {
            return d.HandleStream(ctx)
        }
        return d.HandleStart(ctx)

    case StateRecording:
        return d.HandleStop(ctx)

    case StateStreaming:
        _, err := d.HandleStreamStop(ctx)
        return err

    default:
        return fmt.Errorf("cannot toggle in state: %d", state)
    }
}
```

**Verification**:
```bash
go build ./...
go vet ./...
```

---

### Step 2.3: Update Audio Recorder for Streaming

**File**: `internal/audio/recorder.go`

Add streaming mode that calls a callback for each buffer:

```go
// AudioCallback is called with each audio buffer during streaming
type AudioCallback func(pcmData []byte)

// StartStreaming starts recording and calls the callback with each buffer
func (r *Recorder) StartStreaming(ctx context.Context, callback AudioCallback) error {
    r.mu.Lock()
    if r.recording {
        r.mu.Unlock()
        return fmt.Errorf("already recording")
    }
    r.recording = true
    r.buffer = nil
    r.startTime = time.Now()
    r.mu.Unlock()

    // Initialize PortAudio
    if err := portaudio.Initialize(); err != nil {
        r.mu.Lock()
        r.recording = false
        r.mu.Unlock()
        return fmt.Errorf("portaudio init failed: %w", err)
    }

    // Create input buffer
    inputBuffer := make([]float32, r.framesPerBlock)

    // Open stream
    stream, err := portaudio.OpenDefaultStream(
        1,                  // input channels
        0,                  // output channels
        float64(r.sampleRate),
        r.framesPerBlock,
        inputBuffer,
    )
    if err != nil {
        portaudio.Terminate()
        r.mu.Lock()
        r.recording = false
        r.mu.Unlock()
        return fmt.Errorf("failed to open stream: %w", err)
    }

    if err := stream.Start(); err != nil {
        stream.Close()
        portaudio.Terminate()
        r.mu.Lock()
        r.recording = false
        r.mu.Unlock()
        return fmt.Errorf("failed to start stream: %w", err)
    }

    r.mu.Lock()
    r.stream = stream
    r.mu.Unlock()

    // Recording goroutine
    go func() {
        defer func() {
            stream.Stop()
            stream.Close()
            portaudio.Terminate()
            r.mu.Lock()
            r.recording = false
            r.mu.Unlock()
        }()

        maxDuration := time.Duration(r.maxDurationMin) * time.Minute
        timer := time.NewTimer(maxDuration)
        defer timer.Stop()

        for {
            select {
            case <-ctx.Done():
                return
            case <-timer.C:
                r.errorChan <- fmt.Errorf("max recording duration exceeded")
                return
            default:
            }

            // Check if still recording
            r.mu.Lock()
            recording := r.recording
            r.mu.Unlock()
            if !recording {
                return
            }

            // Read audio
            if err := stream.Read(); err != nil {
                continue // Skip errors, try again
            }

            // Convert to PCM bytes
            pcmData := floatToPCM(inputBuffer)

            // Also append to buffer for potential later use
            r.mu.Lock()
            r.buffer = append(r.buffer, inputBuffer...)
            r.mu.Unlock()

            // Call streaming callback
            callback(pcmData)
        }
    }()

    return nil
}

// floatToPCM converts float32 samples to 16-bit PCM bytes
func floatToPCM(samples []float32) []byte {
    buf := make([]byte, len(samples)*2)
    for i, sample := range samples {
        // Clamp to [-1, 1]
        if sample > 1.0 {
            sample = 1.0
        } else if sample < -1.0 {
            sample = -1.0
        }
        // Convert to int16
        int16Val := int16(sample * 32767)
        buf[i*2] = byte(int16Val)
        buf[i*2+1] = byte(int16Val >> 8)
    }
    return buf
}
```

**Verification**:
```bash
go build ./...
go vet ./...
```

---

### Step 2.4: Update IPC Server

**File**: `internal/ipc/server.go`

Update `processCommand` to handle stream action:

```go
func (s *Server) processCommand(cmd *Command) *Response {
    resp := &Response{
        ID:   cmd.ID,
        Data: make(map[string]string),
    }

    switch cmd.Action {
    // ... existing cases ...

    case ActionStream:
        if err := s.daemon.HandleStream(s.ctx); err != nil {
            resp.Error = err.Error()
        } else {
            resp.Success = true
        }

    // ... rest of cases ...
    }

    // Update state in response
    resp.Data[DataKeyState] = fmt.Sprintf("%d", s.daemon.GetState())
    return resp
}
```

**Verification**:
```bash
go build ./...
go vet ./...
```

---

### Step 2.5: Add CLI Command

**File**: `main.go`

Add stream command:

```go
var streamCmd = &cobra.Command{
    Use:   "stream",
    Short: "Start streaming transcription",
    Long:  "Start real-time streaming transcription (alternative to start/stop)",
    RunE: func(cmd *cobra.Command, args []string) error {
        client := ipc.NewClient()
        resp, err := client.SendCommand(cmd.Context(), ipc.ActionStream, nil)
        if err != nil {
            return err
        }
        if !resp.Success {
            return fmt.Errorf("stream failed: %s", resp.Error)
        }
        fmt.Println("Streaming started")
        return nil
    },
}

func init() {
    rootCmd.AddCommand(streamCmd)
}
```

**Verification**:
```bash
go build ./...
go vet ./...
./build/dictator stream --help
```

---

## Part 3: Overlay (Optional)

### Step 3.1: Create Overlay Project Structure

Create directory structure:

```
overlay/
├── dictator_overlay/
│   ├── __init__.py
│   └── main.py
├── pyproject.toml
└── default.nix
```

**File**: `overlay/pyproject.toml`

```toml
[project]
name = "dictator-overlay"
version = "0.1.0"
description = "GTK4 layer-shell overlay for Dictator streaming preview"
requires-python = ">=3.11"
dependencies = [
    "PyGObject>=3.50.0",
]

[project.scripts]
dictator-overlay = "dictator_overlay.main:main"

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.ruff]
line-length = 100
target-version = "py311"

[tool.ruff.lint]
select = ["E", "F", "I", "UP", "B", "SIM", "RUF"]

[tool.ty]
python-version = "3.11"
```

**Verification**:
```bash
cd overlay
uv sync
uv run ruff check .
```

---

### Step 3.2: Implement Overlay

**File**: `overlay/dictator_overlay/__init__.py`

```python
"""Dictator overlay - GTK4 layer-shell overlay for streaming preview."""

__version__ = "0.1.0"
```

**File**: `overlay/dictator_overlay/main.py`

```python
#!/usr/bin/env python3
"""GTK4 layer-shell overlay for Dictator streaming preview."""

from __future__ import annotations

import json
import os
import socket
import subprocess
from pathlib import Path
from typing import TYPE_CHECKING

import gi

gi.require_version("Gtk", "4.0")

try:
    gi.require_version("Gtk4LayerShell", "1.0")
    HAS_LAYER_SHELL = True
except ValueError:
    HAS_LAYER_SHELL = False

from gi.repository import GLib, Gtk

if HAS_LAYER_SHELL:
    from gi.repository import Gtk4LayerShell

if TYPE_CHECKING:
    from gi.repository import Gio

SOCKET_PATH = "/tmp/dictator-overlay.sock"


class DictatorOverlay(Gtk.Application):
    """GTK4 overlay application for Dictator streaming preview."""

    def __init__(self) -> None:
        super().__init__(application_id="com.dictator.overlay")
        self.window: Gtk.ApplicationWindow | None = None
        self.label: Gtk.Label | None = None
        self.socket_source: int | None = None
        self.server_socket: socket.socket | None = None
        self.client_socket: socket.socket | None = None
        self.text: str = ""
        self.stable_len: int = 0

    def do_activate(self) -> None:
        """Activate the application."""
        self.window = Gtk.ApplicationWindow(application=self)
        self.window.set_title("Dictator")
        self.window.set_default_size(600, 100)

        if HAS_LAYER_SHELL:
            Gtk4LayerShell.init_for_window(self.window)
            Gtk4LayerShell.set_layer(self.window, Gtk4LayerShell.Layer.OVERLAY)
            Gtk4LayerShell.set_keyboard_mode(
                self.window, Gtk4LayerShell.KeyboardMode.ON_DEMAND
            )
            # Position at bottom center
            Gtk4LayerShell.set_anchor(self.window, Gtk4LayerShell.Edge.BOTTOM, True)
            Gtk4LayerShell.set_margin(self.window, Gtk4LayerShell.Edge.BOTTOM, 50)

        # Create UI
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        box.set_margin_start(20)
        box.set_margin_end(20)
        box.set_margin_top(10)
        box.set_margin_bottom(10)

        self.label = Gtk.Label(label="Listening...")
        self.label.set_wrap(True)
        self.label.set_xalign(0)
        self.label.add_css_class("transcript")
        box.append(self.label)

        hint_label = Gtk.Label(label="Enter to confirm | Escape to cancel")
        hint_label.add_css_class("hint")
        box.append(hint_label)

        self.window.set_child(box)

        # Add CSS
        css_provider = Gtk.CssProvider()
        css_provider.load_from_string(
            """
            .transcript {
                font-size: 18px;
                font-weight: 500;
            }
            .hint {
                font-size: 12px;
                opacity: 0.7;
            }
            window {
                background-color: rgba(30, 30, 30, 0.95);
                border-radius: 10px;
            }
            """
        )
        Gtk.StyleContext.add_provider_for_display(
            self.window.get_display(),
            css_provider,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION,
        )

        # Set up keyboard handling
        key_controller = Gtk.EventControllerKey()
        key_controller.connect("key-pressed", self.on_key_pressed)
        self.window.add_controller(key_controller)

        # Start IPC server
        self.start_ipc_server()

        # Position near cursor using Hyprland
        self.position_window()

        self.window.present()

    def position_window(self) -> None:
        """Position window near the focused application using Hyprland IPC."""
        if not HAS_LAYER_SHELL:
            return

        try:
            result = subprocess.run(
                ["hyprctl", "activewindow", "-j"],
                capture_output=True,
                text=True,
                timeout=1,
                check=False,
            )
            if result.returncode == 0 and result.stdout:
                data = json.loads(result.stdout)
                # Window is at data["at"] = [x, y] and data["size"] = [w, h]
                # Position our overlay relative to it
                x = data.get("at", [0, 0])[0]
                _y = data.get("at", [0, 0])[1]
                _w = data.get("size", [0, 0])[0]
                h = data.get("size", [0, 0])[1]

                # Position below the active window
                # Note: Layer shell doesn't support arbitrary positioning,
                # so we use anchors and margins instead
                Gtk4LayerShell.set_anchor(
                    self.window, Gtk4LayerShell.Edge.BOTTOM, False
                )
                Gtk4LayerShell.set_anchor(self.window, Gtk4LayerShell.Edge.TOP, True)
                Gtk4LayerShell.set_anchor(self.window, Gtk4LayerShell.Edge.LEFT, True)
                Gtk4LayerShell.set_margin(
                    self.window, Gtk4LayerShell.Edge.TOP, _y + h + 10
                )
                Gtk4LayerShell.set_margin(self.window, Gtk4LayerShell.Edge.LEFT, x)
        except (subprocess.SubprocessError, json.JSONDecodeError, KeyError):
            pass  # Fall back to default positioning

    def start_ipc_server(self) -> None:
        """Start the Unix socket IPC server."""
        # Remove existing socket
        socket_path = Path(SOCKET_PATH)
        if socket_path.exists():
            socket_path.unlink()

        self.server_socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server_socket.bind(SOCKET_PATH)
        self.server_socket.listen(1)
        self.server_socket.setblocking(False)

        # Watch for connections
        self.socket_source = GLib.io_add_watch(
            self.server_socket.fileno(),
            GLib.IO_IN,
            self.on_socket_connect,
        )

    def on_socket_connect(
        self, _fd: int, _condition: GLib.IOCondition
    ) -> bool:
        """Handle new socket connection."""
        if self.server_socket is None:
            return False

        try:
            self.client_socket, _ = self.server_socket.accept()
            self.client_socket.setblocking(False)
            GLib.io_add_watch(
                self.client_socket.fileno(),
                GLib.IO_IN,
                self.on_socket_data,
            )
        except BlockingIOError:
            pass
        return True

    def on_socket_data(self, _fd: int, _condition: GLib.IOCondition) -> bool:
        """Handle incoming socket data."""
        if self.client_socket is None:
            return False

        try:
            data = self.client_socket.recv(4096)
            if not data:
                self.client_socket.close()
                self.client_socket = None
                return False

            msg = json.loads(data.decode("utf-8"))
            self.handle_message(msg)
        except (BlockingIOError, json.JSONDecodeError):
            pass
        except OSError:
            self.client_socket = None
            return False
        return True

    def handle_message(self, msg: dict) -> None:
        """Handle a message from the daemon."""
        msg_type = msg.get("type")

        if msg_type == "update":
            self.text = msg.get("text", "")
            self.stable_len = msg.get("stable_len", 0)
            self.update_label()

        elif msg_type == "show":
            if self.window:
                self.window.present()

        elif msg_type == "hide":
            if self.window:
                self.window.hide()

    def update_label(self) -> None:
        """Update the label with current text."""
        if self.label is None:
            return

        if not self.text:
            self.label.set_markup("<i>Listening...</i>")
            return

        # Show stable text normally, tentative text in italic
        stable = GLib.markup_escape_text(self.text[: self.stable_len])
        tentative = GLib.markup_escape_text(self.text[self.stable_len :])

        markup = f"{stable}<i>{tentative}</i>"
        self.label.set_markup(markup)

    def on_key_pressed(
        self,
        _controller: Gtk.EventControllerKey,
        keyval: int,
        _keycode: int,
        _state: int,
    ) -> bool:
        """Handle key press events."""
        if keyval == 65293:  # Return/Enter
            self.confirm()
            return True
        elif keyval == 65307:  # Escape
            self.cancel()
            return True
        return False

    def confirm(self) -> None:
        """Confirm the transcription."""
        if self.client_socket:
            try:
                self.client_socket.send(json.dumps({"type": "confirm"}).encode("utf-8"))
            except OSError:
                pass
        self.quit()

    def cancel(self) -> None:
        """Cancel the transcription."""
        if self.client_socket:
            try:
                self.client_socket.send(json.dumps({"type": "cancel"}).encode("utf-8"))
            except OSError:
                pass
        self.quit()

    def do_shutdown(self) -> None:
        """Clean up on shutdown."""
        if self.socket_source:
            GLib.source_remove(self.socket_source)

        if self.client_socket:
            self.client_socket.close()

        if self.server_socket:
            self.server_socket.close()

        socket_path = Path(SOCKET_PATH)
        if socket_path.exists():
            socket_path.unlink()

        Gtk.Application.do_shutdown(self)


def main() -> None:
    """Entry point."""
    app = DictatorOverlay()
    app.run()


if __name__ == "__main__":
    main()
```

**Verification**:
```bash
cd overlay
uv run ty check dictator_overlay/
uv run ruff check dictator_overlay/
uv run ruff format dictator_overlay/
```

---

### Step 3.3: Create Nix Package

**File**: `overlay/default.nix`

```nix
{
  lib,
  python3Packages,
  gtk4,
  gtk4-layer-shell,
  gobject-introspection,
  wrapGAppsHook4,
}:

python3Packages.buildPythonApplication {
  pname = "dictator-overlay";
  version = "0.1.0";
  format = "pyproject";

  src = ./.;

  nativeBuildInputs = [
    gobject-introspection
    wrapGAppsHook4
  ];

  buildInputs = [
    gtk4
    gtk4-layer-shell
  ];

  propagatedBuildInputs = with python3Packages; [
    pygobject3
    hatchling
  ];

  # Don't wrap twice
  dontWrapGApps = true;

  preFixup = ''
    makeWrapperArgs+=("''${gappsWrapperArgs[@]}")
  '';

  meta = with lib; {
    description = "GTK4 layer-shell overlay for Dictator streaming preview";
    license = licenses.mit;
    platforms = platforms.linux;
  };
}
```

**Verification**:
```bash
# Test build with nix
nix-build -E 'with import <nixpkgs> {}; callPackage ./overlay/default.nix {}'
```

---

### Step 3.4: Integrate Overlay with Daemon

**File**: `internal/overlay/overlay.go` (new file)

```go
package overlay

import (
    "encoding/json"
    "fmt"
    "net"
    "os"
    "os/exec"
    "sync"
    "time"
)

const socketPath = "/tmp/dictator-overlay.sock"

// Message types for overlay IPC
type Message struct {
    Type      string `json:"type"`
    Text      string `json:"text,omitempty"`
    StableLen int    `json:"stable_len,omitempty"`
}

// Manager manages the overlay process and communication
type Manager struct {
    cmd    *exec.Cmd
    conn   net.Conn
    connMu sync.Mutex

    onConfirm func()
    onCancel  func()
}

// NewManager creates a new overlay manager
func NewManager() *Manager {
    return &Manager{}
}

// SetHandlers sets the confirm/cancel handlers
func (m *Manager) SetHandlers(onConfirm, onCancel func()) {
    m.onConfirm = onConfirm
    m.onCancel = onCancel
}

// Start spawns the overlay process
func (m *Manager) Start() error {
    // Find overlay binary
    overlayPath, err := exec.LookPath("dictator-overlay")
    if err != nil {
        return fmt.Errorf("overlay not found: %w", err)
    }

    m.cmd = exec.Command(overlayPath)
    m.cmd.Stdout = os.Stdout
    m.cmd.Stderr = os.Stderr

    if err := m.cmd.Start(); err != nil {
        return fmt.Errorf("failed to start overlay: %w", err)
    }

    // Wait for socket to appear
    for i := 0; i < 50; i++ {
        time.Sleep(100 * time.Millisecond)
        if _, err := os.Stat(socketPath); err == nil {
            break
        }
    }

    // Connect to overlay
    conn, err := net.Dial("unix", socketPath)
    if err != nil {
        m.cmd.Process.Kill()
        return fmt.Errorf("failed to connect to overlay: %w", err)
    }

    m.connMu.Lock()
    m.conn = conn
    m.connMu.Unlock()

    // Start receiver
    go m.receiveLoop()

    return nil
}

// Update sends text update to overlay
func (m *Manager) Update(text string, stableLen int) error {
    msg := Message{
        Type:      "update",
        Text:      text,
        StableLen: stableLen,
    }
    return m.send(msg)
}

// Show makes the overlay visible
func (m *Manager) Show() error {
    return m.send(Message{Type: "show"})
}

// Hide hides the overlay
func (m *Manager) Hide() error {
    return m.send(Message{Type: "hide"})
}

// Stop terminates the overlay process
func (m *Manager) Stop() error {
    m.connMu.Lock()
    if m.conn != nil {
        m.conn.Close()
        m.conn = nil
    }
    m.connMu.Unlock()

    if m.cmd != nil && m.cmd.Process != nil {
        m.cmd.Process.Kill()
        m.cmd.Wait()
    }

    return nil
}

func (m *Manager) send(msg Message) error {
    m.connMu.Lock()
    defer m.connMu.Unlock()

    if m.conn == nil {
        return fmt.Errorf("not connected")
    }

    data, err := json.Marshal(msg)
    if err != nil {
        return err
    }

    _, err = m.conn.Write(data)
    return err
}

func (m *Manager) receiveLoop() {
    buf := make([]byte, 4096)

    for {
        m.connMu.Lock()
        conn := m.conn
        m.connMu.Unlock()

        if conn == nil {
            return
        }

        n, err := conn.Read(buf)
        if err != nil {
            return
        }

        var msg Message
        if err := json.Unmarshal(buf[:n], &msg); err != nil {
            continue
        }

        switch msg.Type {
        case "confirm":
            if m.onConfirm != nil {
                m.onConfirm()
            }
        case "cancel":
            if m.onCancel != nil {
                m.onCancel()
            }
        }
    }
}
```

**Verification**:
```bash
go build ./...
go vet ./...
```

---

### Step 3.5: Update Flake for Bundling

**File**: `flake.nix` (update)

Add overlay package and bundle with main package:

```nix
{
  # ... existing inputs ...

  outputs = { self, nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          dictator = pkgs.buildGoModule {
            pname = "dictator";
            version = "0.1.0";
            src = ./.;
            vendorHash = "..."; # Update after adding websocket dependency

            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.portaudio ];
          };

          dictator-overlay = pkgs.callPackage ./overlay/default.nix { };

          default = pkgs.symlinkJoin {
            name = "dictator-full";
            paths = [
              self.packages.${system}.dictator
              self.packages.${system}.dictator-overlay
            ];
          };
        }
      );

      homeManagerModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.services.dictator;
        in
        {
          options.services.dictator = {
            enable = lib.mkEnableOption "Dictator voice typing daemon";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.default;
              description = "The dictator package to use";
            };
          };

          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package ];

            systemd.user.services.dictator = {
              Unit = {
                Description = "Dictator voice typing daemon";
                After = [ "graphical-session.target" ];
              };
              Service = {
                ExecStart = "${cfg.package}/bin/dictator daemon";
                Restart = "on-failure";
              };
              Install = {
                WantedBy = [ "default.target" ];
              };
            };

            # Ensure ydotoold is running for Wayland support
            systemd.user.services.ydotoold = {
              Unit = {
                Description = "ydotool daemon";
              };
              Service = {
                ExecStart = "${pkgs.ydotool}/bin/ydotoold";
                Restart = "on-failure";
              };
              Install = {
                WantedBy = [ "default.target" ];
              };
            };
          };
        };
    };
}
```

**Verification**:
```bash
nix flake check
nix build .#default
```

---

## Part 4: Final Integration

### Step 4.1: Update Streaming Handler for Overlay Mode

**File**: `internal/streaming/handler.go` (update)

Add overlay support:

```go
import (
    "dictator/internal/overlay"
)

type Handler struct {
    // ... existing fields ...
    overlayMode bool
    overlay     *overlay.Manager
}

func NewHandler(client *Client, typer typing.StreamingTyper, overlayMode bool) *Handler {
    h := &Handler{
        client:      client,
        typer:       typer,
        overlayMode: overlayMode,
    }

    if overlayMode {
        h.overlay = overlay.NewManager()
    }

    return h
}

func (h *Handler) Start(ctx context.Context) error {
    // Start overlay if in overlay mode
    if h.overlayMode && h.overlay != nil {
        if err := h.overlay.Start(); err != nil {
            return fmt.Errorf("failed to start overlay: %w", err)
        }

        h.overlay.SetHandlers(
            func() { /* confirm - stop streaming and type */ },
            func() { /* cancel - stop streaming, don't type */ },
        )
    }

    // ... rest of Start ...
}

func (h *Handler) handlePartial(text string, stableLen int, seq int) {
    h.mu.Lock()
    defer h.mu.Unlock()

    h.lastText = text

    if h.overlayMode && h.overlay != nil {
        // Send to overlay for display
        h.overlay.Update(text, stableLen)
    } else {
        // Direct typing mode
        if stableLen > h.typedLen {
            newText := text[h.typedLen:stableLen]
            if err := h.typer.TypeIncremental(context.Background(), newText); err != nil {
                // Log error
            }
            h.typedLen = stableLen
        }
    }
}
```

**Verification**:
```bash
go build ./...
go vet ./...
```

---

### Step 4.2: Update Daemon to Support Overlay Config

**File**: `internal/daemon/daemon.go` (update)

```go
func (d *Daemon) HandleStream(ctx context.Context) error {
    // ... existing setup ...

    // Check if overlay mode
    overlayMode := cfg.Output == "overlay"

    // Create handler with overlay mode
    handler := streaming.NewHandler(client, typer, overlayMode)

    // ... rest of HandleStream ...
}
```

**Verification**:
```bash
go build ./...
go vet ./...
make build
./build/dictator --help
```

---

## Final Verification Checklist

### Go Code
```bash
# Build
go build ./...

# Vet
go vet ./...

# Test (when tests are added)
go test ./...

# Run daemon
make build
./build/dictator daemon
```

### Python Overlay
```bash
cd overlay

# Type check
uv run ty check dictator_overlay/

# Lint
uv run ruff check dictator_overlay/

# Format
uv run ruff format dictator_overlay/

# Run manually
uv run dictator-overlay
```

### Nix Build
```bash
# Check flake
nix flake check

# Build all packages
nix build .#default

# Test home-manager module (in your dotfiles)
home-manager switch
```

### Integration Test
```bash
# Terminal 1: Start daemon in streaming mode
./build/dictator daemon

# Terminal 2: Start streaming
./build/dictator stream

# Speak and verify text appears
# Press toggle keybind or run:
./build/dictator toggle
```

---

## Notes for Implementer

1. **ydotool Setup**: Users need `ydotoold` running with uinput access. Document this in README:
   ```bash
   # Add user to input group
   sudo usermod -aG input $USER
   # Or create udev rule for uinput
   ```

2. **WebSocket Dependency**: After adding gorilla/websocket, run:
   ```bash
   go mod tidy
   ```
   Then update the vendorHash in flake.nix.

3. **Overlay GTK4**: The overlay requires GTK4 and gtk4-layer-shell. These are available in nixpkgs but may need manual installation on non-NixOS systems.

4. **Hyprland Positioning**: The overlay positioning queries Hyprland. For other compositors, this may need adaptation.

5. **Audio Buffer Size**: The streaming client sends audio in buffer-sized chunks (1024 samples = 64ms at 16kHz). This is more frequent than the 560ms transcription chunks, which is fine - the server buffers until it has enough.

6. **Error Handling**: Add proper error propagation and notification updates for streaming errors.

7. **Backwards Compatibility**: The `toggle` command automatically uses the configured mode. Users can explicitly use `start`/`stop` for batch mode or `stream` for streaming mode regardless of config.
