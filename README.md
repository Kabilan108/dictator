# Dictator

[![Rust](https://img.shields.io/badge/rust-2024_edition-orange?style=flat&logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/github/license/kabilan108/dictator)](LICENSE)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/Kabilan108/dictator)
[![Platform](https://img.shields.io/badge/platform-linux-lightgrey.svg)](https://github.com/kabilan108/dictator)

A voice typing daemon for Linux that enables voice typing anywhere the cursor is positioned. Uses Whisper API for speech recognition and provides seamless integration with any application through keyboard input simulation.

## Desktop app

The optional GPUI desktop app provides transcript history, playback, editing and
revision comparison, recording controls, statistics, and a system tray popup.
The daemon continues to own recording, transcription and insertion.

```bash
# Native development build and launch
direnv exec "$PWD" cargo run --features gui --bin dictator-gui

# Isolated sample history for UI inspection
direnv exec "$PWD" cargo run --features gui --bin dictator-gui -- --demo

# Nix GUI package, separate from the headless daemon package
nix run .#gui
```

Use `--tray` to start without opening the history window. Left-click the tray
icon for quick controls; its menu can open history or quit the GUI. Quitting the
GUI leaves the daemon running. The GUI requires a graphical session and a
StatusNotifierItem host for tray integration. The main window also works without
a tray host. Use matching daemon and GUI builds for microphone priorities and
terminal recording updates. On niri the popup is positioned through compositor
IPC; other Wayland compositors may need a floating window rule for
`Dictator quick controls`.

For Home Manager, enable `services.dictator.gui.enable = true` alongside the
existing daemon configuration. GUI autostart defaults to enabled and can be
turned off with `services.dictator.gui.autostart = false`. On compositors that do
not process XDG autostart entries, launch `dictator-gui --tray` from compositor
startup.

The Settings screen displays daemon configuration read-only. Microphone
priorities live in a separate app-owned preferences file; Home Manager's
`config.json` is never rewritten by the GUI. Discovery retains disconnected
devices and their order, appends new devices, and resolves an input when a new
recording begins. On PipeWire/PulseAudio, install the PulseAudio client tools
`pactl` and `parec`; the Nix packages/module provide them.

History keeps a recording's identity and capture time across retries. Attempts
retain their own outcomes and timing. Transcript edits append revisions and
restoring an earlier text creates a new revision. Latency statistics cover
measured transcription requests; older records without measurements do not get
synthetic latency values. See [desktop validation](docs/gui-validation.md) for
the local acceptance checks and remaining platform limits.

## Quick Start

### Prerequisites

Make sure you have the following system dependencies installed:

**For X11:**
```bash
# Ubuntu/Debian
sudo apt install xdotool xclip pulseaudio-utils pkg-config

# Arch Linux
sudo pacman -S xdotool xclip libpulse

# Fedora
sudo dnf install xdotool xclip pulseaudio-utils
```

**For Wayland:**
```bash
# Ubuntu/Debian
sudo apt install wl-clipboard wtype pulseaudio-utils pkg-config

# Arch Linux
sudo pacman -S wl-clipboard wtype libpulse

# Fedora
sudo dnf install wl-clipboard wtype pulseaudio-utils
```

You also need a Rust toolchain (`cargo`, Rust 1.88 or newer). Audio capture uses `pactl` for discovery and `parec` for capture. Run PipeWire with `pipewire-pulse`, or a PulseAudio server. The GUI also needs `ffplay` from FFmpeg for playback. The Nix development shell supplies the native GUI build dependencies.

### Installation

1. **Clone and build:**
   ```bash
   git clone https://github.com/kabilan108/dictator.git
   cd dictator
   make build
   ```

2. **Install for the current user:**
   ```bash
   make install
   ```

3. **Configure API access:**
   ```bash
   dictator init
   config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
   $EDITOR "$config_home/dictator/config.json"

   # The supplied service reads secrets from this fixed path.
   install -d -m 700 ~/.config/dictator
   touch ~/.config/dictator/environment
   chmod 600 ~/.config/dictator/environment
   $EDITOR ~/.config/dictator/environment
   ```

   Add your Whisper API endpoint and an environment-variable reference for the key:
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

   Add the referenced variable to `~/.config/dictator/environment` using systemd
   `EnvironmentFile` syntax (no `export`):
   ```text
   OPENAI_API_KEY=replace-with-your-key
   ```

4. **Set up the systemd user service:**
   ```bash
   mkdir -p "$config_home/systemd/user"
   cp dictator.service "$config_home/systemd/user/dictator.service"
   systemctl --user daemon-reload
   ```

   The unit directory above assumes the shell and systemd user manager use the
   same config root. If they differ, install the unit in the manager's config
   root instead; the daemon config path can be set independently below.

   The supplied unit expects the default Cargo install path,
   `~/.cargo/bin/dictator`. If `make install` used `CARGO_HOME` or
   `CARGO_INSTALL_ROOT`, find the full installed binary path and run
   `systemctl --user edit dictator.service` before enabling the service. Add an
   `ExecStart=` reset followed by the absolute path:
   ```ini
   [Service]
   ExecStart=
   ExecStart=/absolute/path/to/dictator daemon
   ```

   `dictator init` honors `XDG_CONFIG_HOME`. If that variable points somewhere
   other than `~/.config` and the systemd user manager does not already have the
   same value, run `systemctl --user edit dictator.service` and add it as an
   absolute path. If you also changed `ExecStart`, put this line under the same
   `[Service]` heading:
   ```ini
   [Service]
   Environment=XDG_CONFIG_HOME=/absolute/path/to/config-root
   ```

   The supplied unit limits home-directory writes to Dictator's default config,
   data, and state directories. If `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, or
   `XDG_STATE_HOME` points elsewhere, add every custom app directory to the
   same drop-in and replace the preparation command with those exact paths:
   ```ini
   [Service]
   Environment=XDG_CONFIG_HOME=/custom/config
   Environment=XDG_DATA_HOME=/custom/data
   Environment=XDG_STATE_HOME=/custom/state
   ExecStartPre=
   ExecStartPre=+/usr/bin/env install -d -m 0700 /custom/config/dictator /custom/data/dictator /custom/state/dictator
   ReadWritePaths=
   ReadWritePaths=/custom/config/dictator /custom/data/dictator /custom/state/dictator
   ```
   Keep the paths absolute. Resetting both list directives prevents the base
   unit from requiring its default directories after the paths change.

   After adding any needed drop-in, enable the service. This attaches it to
   `graphical-session.target` without starting it during a headless login:
   ```bash
   systemctl --user enable dictator.service
   ```

   A desktop session must import its display environment into the systemd user
   manager before Dictator starts. Many desktop environments already do this
   and activate `graphical-session.target`. Run these commands once from a
   terminal in the current graphical session to start Dictator now. For a custom
   compositor, also add them to its graphical-session startup, in this order:
   ```bash
   systemctl --user import-environment \
     DISPLAY XAUTHORITY WAYLAND_DISPLAY XDG_SESSION_TYPE DBUS_SESSION_BUS_ADDRESS
   if [ -n "${NIRI_SOCKET:-}" ]; then
     systemctl --user import-environment NIRI_SOCKET
   fi
   systemctl --user start dictator.service
   ```

   Run that startup hook from the graphical session, not from a shell profile
   or headless boot. It starts Dictator directly and does not manually start
   `graphical-session.target` on compositors that do not manage that target.

   The service reads `~/.config/dictator/environment` if it exists. Keep that
   file private because it contains the API key. If you run `dictator daemon`
   directly instead, export `OPENAI_API_KEY` in that shell first.

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
- The service sandbox derives its writable config, data, and state directories
  from Home Manager's XDG paths, or from absolute `XDG_CONFIG_HOME`,
  `XDG_DATA_HOME`, and `XDG_STATE_HOME` values in `services.dictator.environment`.
  The effective config root must be within `home.homeDirectory` because Home
  Manager owns `config.json` there.
  If an `environmentFile` is intended to change any XDG root at runtime, add
  direct `Environment`, `ExecStartPre`, and `ReadWritePaths` service overrides
  for the resulting `dictator` directories; the unit's direct XDG assignments
  otherwise take precedence over values from an environment file.

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
systemctl --user status dictator.service

# Start/stop/restart the service
systemctl --user start dictator.service
systemctl --user stop dictator.service
systemctl --user restart dictator.service

# View service logs
journalctl --user -u dictator.service -f
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
| `retry [AUDIO_FILE]` | Retry the latest failed transcription or a saved WAV file |

### Retrying a transcription

```bash
# Retry the most recent failed transcription
dictator retry

# Retry a specific recording, including one saved by an older version
dictator retry ~/.local/share/dictator/recordings/recording.wav

# Save the recovered text to a file
dictator retry > recovered.txt
```

Retry prints the recovered text in the terminal and saves it to transcript history. It preserves the audio file. A successful retry removes that recording from the pending failures, so the next `dictator retry` selects the next most recent failure.

Only one CLI retry can run at a time. Ctrl-C cancels the request and keeps an existing failure available for another attempt. If the provider succeeds but history cannot be saved, the command still prints the recovered text, reports the storage error on stderr, and exits with a nonzero status.

The command contacts the configured transcription provider directly and works without a running daemon. Run it with the same configuration and API key environment variables as the daemon. Variables loaded only by systemd's `EnvironmentFile` are not inherited by your terminal.

Automatic failure tracking starts with this version of the daemon. For recordings from earlier versions, pass the WAV path explicitly. Recordings are stored under `$XDG_DATA_HOME/dictator/recordings`, or `~/.local/share/dictator/recordings` by default.

### Daemon connection

The daemon and CLI communicate through `$XDG_RUNTIME_DIR/dictator/dictator.sock`. If `XDG_RUNTIME_DIR` is unavailable, they use `/tmp/dictator-$UID/dictator.sock` inside a private directory owned by the current user. The Rust CLI does not fall back to the legacy Go socket at `/tmp/dictator.sock`, because that shared path can be claimed by another user. After upgrading from the Go daemon or an earlier Rust build, restart the daemon before using CLI commands or desktop shortcuts so both processes use the new socket.

A CLI response timeout does not cancel a command already delivered to the daemon. Its outcome is unknown and it may still execute; do not automatically retry a mutating command after a timeout.

## Configuration

Configuration file location: `$XDG_CONFIG_HOME/dictator/config.json`, defaulting to `~/.config/dictator/config.json` when `XDG_CONFIG_HOME` is unset or empty.

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

The `api.providers.<name>.key` field supports `${env:VAR_NAME}` substitutions. If the active provider key references missing environment variables, config loading fails. The supplied systemd user service loads variables from `~/.config/dictator/environment` when that file exists.

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
- Config stored in `$XDG_CONFIG_HOME/dictator/config.json`, with `~/.config` as the default config root
