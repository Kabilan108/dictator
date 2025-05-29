# Dictator

[![Go Version](https://img.shields.io/badge/go-1.24+-00ADD8?style=flat&logo=go)](https://golang.org/)
[![License](https://img.shields.io/github/license/kabilan108/dictator)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-linux-lightgrey.svg)](https://github.com/kabilan108/dictator)

A voice typing daemon for Linux that enables voice typing anywhere the cursor is positioned. Uses Whisper API for speech recognition and provides seamless integration with any application through keyboard input simulation.

## 🚀 Quick Start

### Prerequisites

Make sure you have the following system dependencies installed:

```bash
# Ubuntu/Debian
sudo apt install xdotool xclip pulseaudio-utils

# Arch Linux
sudo pacman -S xdotool xclip pulseaudio

# Fedora
sudo dnf install xdotool xclip pulseaudio-utils
```

### Installation

1. **Clone and build:**
   ```bash
   git clone https://github.com/kabilan108/dictator.git
   cd dictator
   make build
   ```

2. **Install to system (optional):**
   ```bash
   make install
   ```

3. **Configure API access:**
   ```bash
   # Edit the config file that gets created automatically
   ~/.config/dictator/config.json
   ```

   Add your Whisper API endpoint and key:
   ```json
   {
     "api": {
       "endpoint": "https://api.openai.com/v1/audio/transcriptions",
       "key": "your-api-key-here",
       "model": "whisper-1",
       "timeout": 60
     }
   }
   ```

### Basic Usage

1. **Start the daemon:**
   ```bash
   dictator daemon
   ```

2. **In another terminal, control voice recording:**
   ```bash
   # Start recording
   dictator start

   # Stop recording and transcribe
   dictator stop

   # Toggle recording on/off
   dictator toggle

   # Cancel current operation
   dictator cancel

   # Check daemon status
   dictator status
   ```

## 📖 Usage

### Daemon Mode

The daemon runs in the background and handles all audio recording, transcription, and typing operations:

```bash
# Run daemon in foreground (recommended for development)
dictator daemon

# Or run in background
nohup dictator daemon > /dev/null 2>&1 &
```

### CLI Commands

| Command | Description |
|---------|-------------|
| `start` | Begin voice recording |
| `stop` | Stop recording and start transcription |
| `toggle` | Toggle between recording and idle states |
| `cancel` | Cancel any ongoing operation |
| `status` | Show daemon status and uptime |

### Workflow

1. **Start Recording**: Use `dictator start` or `dictator toggle`
2. **Speak**: Talk into your microphone (you'll see a notification)
3. **Stop Recording**: Use `dictator stop` or `dictator toggle`
4. **Automatic Transcription**: The daemon sends audio to Whisper API
5. **Text Insertion**: Transcribed text is typed at your cursor position

## ⚙️ Configuration

Configuration file location: `~/.config/dictator/config.json`

### Example Configuration

```json
{
  "api": {
    "endpoint": "https://api.openai.com/v1/audio/transcriptions",
    "key": "sk-your-api-key",
    "model": "whisper-1",
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
    "log_level": 1,
    "max_recording_min": 5,
    "typing_delay_ms": 10
  }
}
```

### Configuration Options

#### API Settings
- `endpoint`: Whisper API endpoint URL
- `key`: Your API key
- `model`: Whisper model to use (e.g., "whisper-1", "distil-large-v3")
- `timeout`: Request timeout in seconds

#### Audio Settings
- `sample_rate`: Audio sample rate in Hz (default: 16000)
- `channels`: Number of audio channels (default: 1)
- `bit_depth`: Audio bit depth (default: 16)
- `frames_per_block`: Audio buffer size (default: 1024)
- `max_duration_min`: Maximum recording duration in minutes

#### App Settings
- `log_level`: Log verbosity (0=debug, 1=info, 2=warn, 3=error)
- `max_recording_min`: Recording limit in minutes
- `typing_delay_ms`: Delay between keystrokes when typing

## 🏗️ Architecture

Dictator uses a client-server architecture with these key components:

```
┌─────────────┐    Unix Socket    ┌─────────────────┐
│    CLI      │ ←─────IPC────────→ │     Daemon      │
│  Commands   │                   │                 │
└─────────────┘                   └─────────────────┘
                                           │
                ┌──────────────────────────┼──────────────────────────┐
                │                          │                          │
        ┌───────▼────────┐     ┌──────────▼──────────┐     ┌─────────▼────────┐
        │  Audio         │     │   Transcription     │     │    Typing        │
        │  Recording     │     │   (Whisper API)     │     │   (xdotool)      │
        │  (PortAudio)   │     │                     │     │                  │
        └────────────────┘     └─────────────────────┘     └──────────────────┘
                │                          │                          │
        ┌───────▼────────┐     ┌──────────▼──────────┐     ┌─────────▼────────┐
        │ Notifications  │     │    File Storage     │     │   Clipboard      │
        │    (dunst)     │     │    (cache dir)      │     │   (fallback)     │
        └────────────────┘     └─────────────────────┘     └──────────────────┘
```

### State Machine

The daemon operates as a state machine:

```
     start        stop/timeout     transcription      typing
Idle ────→ Recording ─────────→ Transcribing ────→ Typing ────→ Idle
  ▲                                                              │
  │                           cancel                             │
  └──────────────────────────────────────────────────────────────┘
```

## 🛠️ Development

### Building from Source

```bash
# Install dependencies
go mod download

# Build binary
make build

# Run tests (when available)
make test

# Clean build artifacts
make clean

# Update dependencies
make deps
```

## 🐛 Troubleshooting

### Common Issues

**Daemon won't start:**
- Check if another instance is running: `ps aux | grep dictator`
- Verify audio permissions: `ls -la /dev/snd/`
- Check socket permissions: `ls -la /tmp/dictator.sock`

**Audio recording fails:**
- Install PortAudio: `sudo apt install portaudio19-dev`
- Check microphone access: `arecord -l`
- Verify PulseAudio is running: `pulseaudio --check`

**Text isn't being typed:**
- Install xdotool: `sudo apt install xdotool`
- Check X11 session: `echo $DISPLAY`
- Try clipboard fallback: `dictator` will automatically use xclip if xdotool fails

**API errors:**
- Verify API key in config: `~/.config/dictator/config.json`
- Check network connectivity to API endpoint
- Ensure API quota/billing is sufficient

**Notifications not showing:**
- Install dunst: `sudo apt install dunst`
- Start notification daemon: `dunst &`
- Check D-Bus session: `dbus-launch --sh-syntax`

### Debug Mode

Enable debug logging in your config:

```json
{
  "app": {
    "log_level": 0
  }
}
```

### Log Files

- Daemon logs to stderr (capture with `dictator daemon 2> daemon.log`)
- Audio recordings cached in `~/.cache/dictator/recordings/`
- Config stored in `~/.config/dictator/config.json`

## 🙏 Acknowledgments

- [Whisper](https://openai.com/research/whisper) by OpenAI for speech recognition
- [PortAudio](http://www.portaudio.com/) for cross-platform audio I/O
- [Cobra](https://github.com/spf13/cobra) for CLI framework
- [dunst](https://dunst-project.org/) for desktop notifications

---
