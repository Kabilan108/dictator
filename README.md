# Dictator

[![Go Version](https://img.shields.io/badge/go-1.24+-00ADD8?style=flat&logo=go)](https://golang.org/)
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
```

# List recent transcripts
dictator transcripts

# List last 5 transcripts
dictator transcripts -n 5

# Output only text for piping
dictator transcripts -t

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

## Configuration

Configuration file location: `~/.config/dictator/config.json`

### Example Configuration

```json
{
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
  }
}
```

The `api.providers.<name>.key` field supports `${env:VAR_NAME}` substitutions. If the active provider key references missing environment variables, config loading fails.

## Development

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

### Debug Mode



### Log Files

- Daemon logs to stderr (capture with `dictator daemon 2> daemon.log`)
- Application logs stored in `~/.local/state/dictator/app.log`
- Audio recordings stored in `~/.local/share/dictator/recordings/`
- Database stored in `~/.local/share/dictator/transcripts.db`
- Config stored in `~/.config/dictator/config.json`
