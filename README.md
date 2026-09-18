# Dictator

[![Rust](https://img.shields.io/badge/rust-2024_edition-orange?style=flat&logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/github/license/kabilan108/dictator)](LICENSE)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/Kabilan108/dictator)
[![Platform](https://img.shields.io/badge/platform-linux-lightgrey.svg)](https://github.com/kabilan108/dictator)

A voice typing daemon for Linux that enables voice typing anywhere the cursor is positioned. Uses Whisper API for speech recognition and provides seamless integration with any application through keyboard input simulation.

## Quick Start

### Prerequisites

Make sure you have the following system dependencies installed:

**For X11:**
```bash
# Ubuntu/Debian
sudo apt install xdotool xclip libasound2-dev pkg-config

# Arch Linux
sudo pacman -S xdotool xclip alsa-lib

# Fedora
sudo dnf install xdotool xclip alsa-lib-devel
```

**For Wayland:**
```bash
# Ubuntu/Debian
sudo apt install wl-clipboard wtype libasound2-dev pkg-config

# Arch Linux
sudo pacman -S wl-clipboard wtype alsa-lib

# Fedora
sudo dnf install wl-clipboard wtype alsa-lib-devel
```

You also need a Rust toolchain (`cargo`, Rust 1.88 or newer). Audio capture uses ALSA and works through PipeWire or PulseAudio via their ALSA plugins.

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
   dictator init
   $EDITOR ~/.config/dictator/config.json
   ```

   Add your Whisper API endpoint and key:
   ```json
   {
     "enable_osd": true,
     "notifications": "errors_only",
     "api": {
       "active_provider": "openai",
       "timeout": 60,
       "providers": {
         "openai": {
           "endpoint": "https://api.openai.com/v1/audio/transcriptions",
           "key": "${env:OPENAI_API_KEY}",
           "model": "gpt-4o-transcribe"
         }
       }
     }
   }
   ```

### Home Manager (Nix)

You can enable Dictator as a Home Manager service via this flake.

Example `flake.nix` usage:
```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    home-manager.url = "github:nix-community/home-manager";
    home-manager.inputs.nixpkgs.follows = "nixpkgs";
    dictator.url = "github:kabilan108/dictator";
  };

  outputs = { self, nixpkgs, home-manager, dictator, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in
    {
      homeConfigurations."your-user" = home-manager.lib.homeManagerConfiguration {
        inherit pkgs;
        modules = [
          dictator.homeManagerModules.dictator
          {
            services.dictator = {
              enable = true;
              displayServer = "wayland"; # or "x11" / "auto"
              logLevel = "INFO";
              settings = {
                api = {
                  active_provider = "openai";
                  timeout = 60;
                  providers = {
                    openai = {
                      endpoint = "https://api.openai.com/v1/audio/transcriptions";
                      key = "\${env:OPENAI_API_KEY}";
                      model = "gpt-4o-transcribe";
                    };
                  };
                };
                enable_osd = true;
                notifications = "errors_only";
                audio = {
                  max_duration_min = 20;
                };
              };
            };
          }
        ];
      };
    };
}
```

Notes:
- `services.dictator.settings` or `services.dictator.configFile` is required when enabling the module.
- `displayServer` controls the default runtime dependencies and environment (Wayland vs X11).
- If you already manage a config file, set `services.dictator.configFile = /path/to/config.json;`.
- To use `${env:VAR}` in the config, set `services.dictator.environmentFile` (supports strings like `${XDG_RUNTIME_DIR}/...`) or `services.dictator.environment`.

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

# List recent transcripts
dictator transcripts

# List last 5 transcripts
dictator transcripts -n 5

# Output only text for piping
dictator transcripts -t
```

## Usage

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
| `transcripts` | Manage transcript history |

The daemon and CLI communicate through `$XDG_RUNTIME_DIR/dictator/dictator.sock`. If `XDG_RUNTIME_DIR` is unavailable, they use `/tmp/dictator-$UID/dictator.sock` inside a private directory owned by the current user. The Rust CLI does not fall back to the legacy Go socket at `/tmp/dictator.sock`, because that shared path can be claimed by another user. After upgrading from the Go daemon or an earlier Rust build, restart the daemon before using CLI commands or desktop shortcuts so both processes use the new socket.

A CLI response timeout does not cancel a command already delivered to the daemon. Its outcome is unknown and it may still execute; do not automatically retry a mutating command after a timeout.

## Configuration

Configuration file location: `~/.config/dictator/config.json`

### Example Configuration

```json
{
  "enable_osd": true,
  "notifications": "errors_only",
  "api": {
    "active_provider": "openai",
    "timeout": 60,
    "providers": {
      "openai": {
        "endpoint": "https://api.openai.com/v1/audio/transcriptions",
        "key": "${env:OPENAI_API_KEY}",
        "model": "gpt-4o-transcribe"
      }
    }
  },
  "audio": {
    "sample_rate": 16000,
    "channels": 1,
    "bit_depth": 16,
    "frames_per_block": 1024,
    "max_duration_min": 5
  },
  "typing": {
    "shortcut": "ctrl_shift_v",
    "niri_app_shortcuts": {
      "com.t3tools.T3Code": "ctrl_v"
    }
  }
}
```

Audio output is mono 16-bit PCM; `channels` must be `1` and `bit_depth` must be `16`. Capture is limited to 32 Mi samples, about 34 minutes at 16 kHz. Reaching `max_duration_min` stops capture and transcribes the recording.

The `api.providers.<name>.key` field supports `${env:VAR_NAME}` substitutions. If the active provider key references missing environment variables, config loading fails.

The `typing.shortcut` field selects the default simulated paste shortcut. It accepts `"ctrl_v"` or `"ctrl_shift_v"` and defaults to `"ctrl_shift_v"`. Under Niri, `typing.niri_app_shortcuts` can override that shortcut for the focused application's Niri `app_id`; the example uses Ctrl+V for T3 Code. Applications without an override use `typing.shortcut`, and X11 always uses `typing.shortcut`.

The `notifications` field controls desktop notifications:

| Value | Behavior |
|-------|----------|
| `"all"` | Notify for idle, recording, transcribing, typing, and error states |
| `"errors_only"` | Notify only when an operation fails |
| `"off"` | Disable desktop notifications |

When `enable_osd` is true, the daemon emits visual OSD events on `$XDG_RUNTIME_DIR/dictator/osd.sock`, falling back to `/tmp/dictator-$UID/osd.sock` when `XDG_RUNTIME_DIR` is unavailable.

### Visual OSD Events

The OSD socket emits newline-delimited JSON. A new client receives a current state snapshot immediately after connecting.

```json
{"type":"state","value":"recording","recording_duration_ms":0}
{"type":"meter","rms":0.03,"peak":0.2}
{"type":"state","value":"transcribing","recording_duration_ms":4820}
{"type":"state","value":"typing"}
{"type":"state","value":"error","message":"transcription failed"}
{"type":"state","value":"idle"}
```

State events are delivered in order to healthy connected clients. Meter events are best effort: if the OSD falls behind, Dictator keeps only the latest pending meter sample.

A production-ready QuickShell reference client is available in `examples/quickshell-osd`.

## Development

### Building from Source

```bash
# Enter a shell with the toolchain and native deps (optional, needs nix)
nix develop

# Build release binary (build/dictator)
make build

# Run unit and integration tests
make test

# rustfmt + clippy
make check

# Clean build artifacts
make clean

# Update dependencies
make deps
```

### Debug Mode

Run `dictator --log-level DEBUG daemon` for diagnostic output.


### Log Files

- Daemon logs to stderr (capture with `dictator daemon 2> daemon.log`)
- Application logs stored in `~/.local/state/dictator/app.log`
- Audio recordings stored in `~/.local/share/dictator/recordings/`
- Database stored in `~/.local/share/dictator/app.db`
- Config stored in `~/.config/dictator/config.json`
