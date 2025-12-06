# Dictator

[![Go Version](https://img.shields.io/badge/go-1.24+-00ADD8?style=flat&logo=go)](https://golang.org/)
[![License](https://img.shields.io/github/license/kabilan108/dictator)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-linux-lightgrey.svg)](https://github.com/kabilan108/dictator)

A voice typing daemon for Linux that enables voice typing anywhere the cursor is positioned. Uses Whisper API for speech recognition and provides seamless integration with any application through keyboard input simulation.

## 🚀 Quick Start

### Prerequisites

Make sure you have the following system dependencies installed:

**For X11:**
```bash
# Ubuntu/Debian
sudo apt install xdotool xclip portaudio19-dev

# Arch Linux
sudo pacman -S xdotool xclip portaudio

# Fedora
sudo dnf install xdotool xclip portaudio-devel
```

**For Wayland:**
```bash
# Ubuntu/Debian
sudo apt install wl-clipboard wtype portaudio19-dev

# Arch Linux
sudo pacman -S wl-clipboard wtype portaudio

# Fedora
sudo dnf install wl-clipboard wtype portaudio-devel
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

3. **Set up as systemd service:**

   **For traditional Linux distributions:**
   ```bash
   # Copy the service file
   sudo cp dictator.service /etc/systemd/system/dictator@.service

   # Reload systemd and enable the service for your user
   sudo systemctl daemon-reload
   sudo systemctl enable dictator@$USER.service

   # Start the service
   sudo systemctl start dictator@$USER.service
   ```

4. **Configure API access:**
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

```bash
# The daemon runs automatically in the background
# Control voice recording with CLI commands:

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

# You can also run the service manually:
dictator daemon
```

## 📖 Usage

### Daemon Mode

The daemon runs in the background and handles all audio recording, transcription, and typing operations.

#### Using systemd:
```bash
# Check service status
sudo systemctl status dictator@$USER.service

# Start/stop/restart the service
sudo systemctl start dictator@$USER.service
sudo systemctl stop dictator@$USER.service
sudo systemctl restart dictator@$USER.service

# View service logs
journalctl -u dictator@$USER.service -f
```

#### Manual execution:
```bash
# Run daemon in foreground
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
    "max_recording_min": 5
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
- `max_recording_min`: Recording limit in minutes

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
        │  Audio         │     │   Transcription     │     │   Clipboard +    │
        │  Recording     │     │   (Whisper API)     │     │   Paste          │
        │  (PortAudio)   │     │                     │     │                  │
        └────────────────┘     └─────────────────────┘     └──────────────────┘
                │                          │                          │
        ┌───────▼────────┐     ┌──────────▼──────────┐     ┌─────────▼────────┐
        │ Notifications  │     │    File Storage     │     │ X11: xclip +     │
        │    (dbus)      │     │    (cache dir)      │     │      xdotool     │
        └────────────────┘     └─────────────────────┘     │ Wayland: wl-copy │
                                                          │      + wtype     │
                                                          └──────────────────┘
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
- If using systemd: `sudo systemctl status dictator@$USER.service`
- Check systemd logs: `journalctl -u dictator@$USER.service -n 20`
- Verify audio permissions: `ls -la /dev/snd/`
- Check socket permissions: `ls -la /tmp/dictator.sock`

**Audio recording fails:**
- Install PortAudio: `sudo apt install portaudio19-dev`
- Check microphone access: `arecord -l`
- Verify PulseAudio is running: `pulseaudio --check`

**Text isn't being pasted:**
- For X11: Install xdotool and xclip: `sudo apt install xdotool xclip`
- For Wayland: Install wtype and wl-clipboard: `sudo apt install wtype wl-clipboard`
- Check your session type: `echo $XDG_SESSION_TYPE`
- Note: wtype only works on wlroots-based compositors (Sway, Hyprland, etc.)

**API errors:**
- Verify API key in config: `~/.config/dictator/config.json`
- Check network connectivity to API endpoint
- Ensure API quota/billing is sufficient

**Notifications not showing:**
- Ensure a notification daemon is running (e.g., dunst, mako, fnott)
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
- Application logs stored in `~/.local/state/dictator/app.log`
- Audio recordings stored in `~/.local/share/dictator/recordings/`
- Database stored in `~/.local/share/dictator/transcripts.db`
- Config stored in `~/.config/dictator/config.json`

## 🙏 Acknowledgments

- [Whisper](https://openai.com/research/whisper) by OpenAI for speech recognition
- [PortAudio](http://www.portaudio.com/) for cross-platform audio I/O
- [Cobra](https://github.com/spf13/cobra) for CLI framework
- [dunst](https://dunst-project.org/) for desktop notifications

---
