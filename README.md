# Dictator

[![Rust](https://img.shields.io/badge/rust-2024_edition-orange?style=flat&logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/github/license/kabilan108/dictator)](LICENSE)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/Kabilan108/dictator)
[![Platform](https://img.shields.io/badge/platform-linux-lightgrey.svg)](https://github.com/kabilan108/dictator)

A voice typing daemon for Linux that enables voice typing anywhere the cursor is positioned. Uses Whisper API for speech recognition and provides seamless integration with any application through keyboard input simulation.

## Desktop app

The optional desktop app, drawn with [Rune](https://github.com/anomalyco/rune),
provides transcript history, playback, editing and revision comparison,
recording controls, statistics, and the settings that can change while the
daemon runs. The daemon continues to own recording, transcription and insertion.

```bash
# Native development build and launch
direnv exec "$PWD" cargo run --features gui --bin dictator-gui

# Isolated sample history for UI inspection
direnv exec "$PWD" cargo run --features gui --bin dictator-gui -- --demo

# Nix GUI package, separate from the headless daemon package
nix run .#gui

# Headless render of a fixture scene, with no window (see --list-scenes)
direnv exec "$PWD" cargo run --features gui --bin dictator-gui -- \
  --demo --snap /tmp/settings.png --scene settings --theme dark
```

The GUI is a plain windowed app: open it on demand and it exits when its
window closes. The daemon keeps running either way. The desktop shell's
dictation panel provides the bar indicator, quick recording controls, and the
launcher for the GUI. Use matching daemon and GUI builds for microphone
priorities and terminal recording updates.

For Home Manager, enable `services.dictator.gui.enable = true` alongside the
existing daemon configuration to install `dictator-gui`.

The window opens floating at its own size on tiling compositors (niri, sway,
Hyprland), so it needs no window rule, and it follows the system's light or
dark appearance.

The Settings screen switches the transcription provider, orders and forgets
microphones, and edits the glossary; each change applies to the next recording
with no restart. The remaining daemon configuration is shown read-only. Microphone
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

## Install with Nix

The flake provides CLI and GUI packages for `x86_64-linux`. Enable flakes and
install the packages:

```sh
nix profile install github:kabilan108/dictator#default github:kabilan108/dictator#gui
dictator version
```

For a specific release, use `github:kabilan108/dictator/vVERSION#default` and
`github:kabilan108/dictator/vVERSION#gui`, replacing `VERSION` with its number.
GUI packages require a release after the historical `v2.4.0` tag.
Nix builds packages that are not already available from your configured binary
caches.

NixOS/Home Manager users can keep these packages in their configuration
instead of a user profile. Import `inputs.dictator.homeManagerModules.default`
and configure `services.dictator`; enable `services.dictator.gui.enable` for
the desktop app. Retain Dictator's own nixpkgs pin for reproducible builds.
See [builds and releases](docs/releases.md) for the workflow and version bumps.

## Install the Linux CLI archive

New releases provide an x86_64 Linux CLI tarball with checksums, built outside
Nix. See [archive installation](docs/cli-install.md) for supported systems and
runtime dependencies. The GUI is distributed through Nix.

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

For a source build, you also need Python 3.11 or newer and a Rust toolchain (`cargo`, Rust 1.88 or newer). Audio capture uses `pactl` for discovery and `parec` for capture. Run PipeWire with `pipewire-pulse`, or a PulseAudio server. The GUI also needs `ffplay` from FFmpeg for playback. The Nix development shell supplies the native GUI build dependencies.

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
- `settings` is written to `config.json` as is, so provider options such as `send_vocabulary = true;` pass through. `api.active_provider` is the default provider; `dictator provider use` can switch to another configured provider without a rebuild (see [Switching providers](#switching-providers)). The glossary itself lives in the data directory, outside Home Manager's control (see [Vocabulary glossary](#vocabulary-glossary)).
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
| `status` | Show daemon status, uptime, microphone state and provider |
| `mic release` | Close the warm microphone, for example before locking the screen |
| `mic resume` | Reopen the warm microphone after `mic release` |
| `transcripts` | Manage transcript history |
| `retry [AUDIO_FILE]` | Retry the latest failed transcription or a saved WAV file |
| `glossary list` | Print the vocabulary glossary, one term per line |
| `glossary add <TERM>` | Add a term to the glossary; quote terms with spaces |
| `glossary remove <TERM>` | Remove a term, ignoring case |
| `glossary rename <FROM> <TO>` | Replace a term in place, keeping its position |
| `provider [list]` | List the configured providers, marking the active one and the default |
| `provider use <NAME>` | Switch the daemon to another configured provider |
| `provider check [NAME]` | Test a provider's key and endpoint (the active one by default) without sending audio |

### Microphone capture

While the session is unlocked, the daemon keeps one `parec` stream open on the selected microphone and holds the last 10 seconds in memory. A recording starts 450 ms before the start command reached the daemon and ends 200 ms after the stop command, so a first or last word spoken right at the keypress survives. If the warm stream is closed or broken when a recording starts, the daemon opens a new capture as before; that recording has no pre-roll.

The microphone is the first connected input in the priority list (see the desktop app's settings), then the system default. Bluetooth inputs are never used, even when listed first or set as the default. If only Bluetooth inputs are connected, the recording fails with `no non-Bluetooth microphone available`. Before a recording uses the warm stream, and whenever inputs change, the daemon asks the sound server which source the warm stream and the recording's stream are recording from. PulseAudio moves a stream to the default source when its microphone is unplugged; a stream found on any source but its own is closed, its buffer is dropped, and a recording using it fails. Plugging in a microphone, removing one, or changing the default source moves the warm stream to the new selection within about a second. A priority change in the desktop app takes effect at the next recording.

The priority list lives in `$XDG_CONFIG_HOME/dictator/preferences.json`. A USB microphone is identified by its vendor id, product id and serial number, so it keeps its place when plugged into another port. Lists saved by earlier versions identified USB devices by port, and a device moved between ports appeared once per port. The first time the daemon or the desktop app sees such a device connected, its entry takes the new id and keeps the rank of the entry saved for its current port, or else of the highest-ranked old entry with the same name. Other old entries are left alone and show as disconnected, because a port id cannot tell one device moved between ports from two units of one model; remove the stale ones in the desktop app's Settings. Two identical devices connected at once stay separate entries; the second gets its port appended to its id. Built-in and Bluetooth inputs keep their ids. A disconnected microphone can be removed from the list in the desktop app; a connected one cannot, because the next discovery would add it back.

Close the warm stream while the screen is locked by wrapping the locker:

```bash
dictator mic release
swaylock    # blocks until unlocked
dictator mic resume
```

Run swaylock without `-f` here: `-f` returns as soon as the screen locks, which would reopen the microphone immediately. swayidle's `unlock` hook runs on logind's `Unlock` signal, which swaylock does not send, so it cannot carry `mic resume`. `mic release` returns once the stream is closed. A recording in progress keeps capturing until it stops or reaches `max_duration_min`.

The daemon also closes the stream on suspend and reopens it on resume through logind's `PrepareForSleep` signal; this needs no configuration. A `mic release` stays in effect across suspend, and a daemon restart clears it.

The daemon discards a recording without contacting the provider when the time from start to stop is under 300 ms, or when its loudest 30 ms window before the tail is quieter than -50 dBFS after removing DC offset. The level is relative to a full-scale amplitude of 1.0. A discarded recording is not pasted, saved to `recordings/`, or added to history. The OSD returns to idle, no error is shown, and the daemon logs `recording discarded` at INFO with the measured duration and level.

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

When the daemon cannot connect to the provider at all, it tries once more after one second. It never resends after a timeout or after the request was sent, because the provider may still be working on the first copy.

Only one CLI retry can run at a time. Ctrl-C cancels the request and keeps an existing failure available for another attempt. If the provider succeeds but history cannot be saved, the command still prints the recovered text, reports the storage error on stderr, and exits with a nonzero status.

The command contacts the transcription provider directly and works without a running daemon. It uses the provider chosen with `dictator provider use` when that provider's key is available in your environment, otherwise the config default. Run it with the same configuration and API key environment variables as the daemon. Variables loaded only by systemd's `EnvironmentFile` are not inherited by your terminal.

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

Audio output is mono 16-bit PCM; `channels` must be `1` and `bit_depth` must be `16`. Capture is limited to 32 Mi samples, about 34 minutes at 16 kHz. Reaching `max_duration_min` stops capture and transcribes the recording. The recording buffer grows with the recording, so a long `max_duration_min` costs no memory until it is used; the warm stream itself holds 10 seconds (640 KB at 16 kHz).

The `api.providers.<name>.key` field supports `${env:VAR_NAME}` substitutions. If the default provider's key references missing environment variables, config loading fails. Other providers may reference unset variables; they cannot be selected until the variable is set. The supplied systemd user service loads variables from `~/.config/dictator/environment` when that file exists.

### Switching providers

`api.active_provider` is the default. To use another configured provider, for example when your own server is down, switch the running daemon:

```bash
dictator provider            # list providers; * marks the active one
dictator provider use openai # use openai from the next transcription on
dictator provider use siren  # back to the default
```

To find out whether a provider's key works before you record, check it. The daemon sends the provider a request with the key and no audio, so nothing is transcribed:

```bash
dictator provider check        # the active provider
dictator provider check openai # openai: key rejected (HTTP 401, 231 ms): Incorrect API key provided: ...
```

The desktop app runs the same check when it starts, when you switch providers and when you press Check in Settings.

The desktop app's settings offer the same choice. A provider can be selected when it has an endpoint and a key, and every `${env:VAR}` in its key is set in the daemon's environment; the list shows the reason for any provider that cannot be selected. The switch applies to the next transcription and to retries. A recording already being transcribed finishes with the provider it started with.

The daemon saves the choice in `$XDG_CONFIG_HOME/dictator/runtime.json`, which it may write even when Home Manager owns `config.json`, and keeps it across restarts:

```json
{"version": 1, "active_provider": "openai"}
```

Selecting the default again deletes the override, so a later change to `api.active_provider` takes effect. If `runtime.json` names a provider that is no longer configured or cannot be used, the daemon logs a warning and starts with the default.

The `typing.shortcut` field selects the default simulated paste shortcut. It accepts `"ctrl_v"` or `"ctrl_shift_v"` and defaults to `"ctrl_shift_v"`. Under Niri, `typing.niri_app_shortcuts` can override that shortcut for the focused application's Niri `app_id`; the example uses Ctrl+V for T3 Code, where Ctrl+Shift+V pastes twice. Applications without an override use `typing.shortcut`, and X11 always uses `typing.shortcut`. The daemon finds the compositor through `NIRI_SOCKET` when set, otherwise through `$XDG_RUNTIME_DIR/niri.<WAYLAND_DISPLAY>.*.sock`, so the override also works from a systemd service that does not import `NIRI_SOCKET`.

The `notifications` field controls desktop notifications:

| Value | Behavior |
|-------|----------|
| `"all"` | Notify for idle, recording, transcribing, typing, and error states |
| `"errors_only"` | Notify only when an operation fails |
| `"off"` | Disable desktop notifications |

### Vocabulary glossary

[siren](https://github.com/Kabilan108/siren) can boost the spelling of a list of terms: names, tools, jargon. Dictator keeps that list in `$XDG_DATA_HOME/dictator/glossary.json` (default `~/.local/share/dictator/glossary.json`), outside the config directory, so you can edit it while Home Manager owns `config.json`. Edit it from the Glossary section of the desktop app's settings or with `dictator glossary add|remove|rename|list`:

```json
{
  "version": 1,
  "terms": ["Claude", "tmux", "Sietch"]
}
```

A provider receives the list only when it sets `send_vocabulary`, because other providers may reject an unknown form field:

```json
"siren": {
  "endpoint": "https://siren.example/v1/audio/transcriptions",
  "key": "${env:SIREN_API_KEY}",
  "model": "nvidia/parakeet-tdt-0.6b-v2",
  "send_vocabulary": true
}
```

With the flag on, every request, including CLI retries and retries from the desktop app's history, carries the list as a JSON array in the `vocabulary` form field:

| Glossary file | Field sent | Effect in siren |
|---------------|------------|-----------------|
| Missing | none | The server's default list |
| `"terms": []` | `[]` | Boosting off |
| Terms | `["Claude", ...]` | Boosts those terms |

Removing the last term leaves an empty list, which turns boosting off. Delete the file to go back to the server's default list.

The daemon checks the file before each request and rereads it only when it changed, so edits apply to the next transcription without a restart. Before sending, it trims each term and drops anything siren would reject with a 400, logging a warning: empty terms, terms over 64 characters, terms with control or invisible characters, case-insensitive duplicates, and terms past the 200th. If the file cannot be parsed, the daemon logs a warning and sends no field. The CLI and the desktop app refuse invalid terms when you add them.

When `enable_osd` is true, the daemon emits visual OSD events on `$XDG_RUNTIME_DIR/dictator/osd.sock`, falling back to `/tmp/dictator-$UID/osd.sock` when `XDG_RUNTIME_DIR` is unavailable.

### Visual OSD Events

The OSD socket emits newline-delimited JSON. A new client receives a current state snapshot immediately after connecting.

```json
{"type":"state","value":"recording","recording_duration_ms":0,"provider":"openai"}
{"type":"meter","rms":0.03,"peak":0.2}
{"type":"state","value":"transcribing","recording_duration_ms":4820}
{"type":"state","value":"typing","text":"Ship the overlay behind a feature flag first."}
{"type":"state","value":"error","message":"transcription failed"}
{"type":"state","value":"idle"}
```

`provider` names the provider the recording will be transcribed with, chosen
when it started. `text` is the text being pasted. Both are omitted when empty,
as are the other optional fields, and a client should ignore fields it does
not know. The socket is readable only by your user, which is why it may carry
transcript text.

A recording that the daemon discards as too short or silent goes from `transcribing` straight to `idle`.

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
- Vocabulary glossary stored in `~/.local/share/dictator/glossary.json`
- Config stored in `$XDG_CONFIG_HOME/dictator/config.json`, with `~/.config` as the default config root
- Provider override stored in `$XDG_CONFIG_HOME/dictator/runtime.json`, microphone priority in `preferences.json` next to it
