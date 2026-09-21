# Linux CLI archive

This archive contains the Dictator CLI for x86_64 Linux with glibc 2.35 or
newer, such as Ubuntu 22.04 and newer. It does not require Nix or Rust.
The GUI is available through the Nix flake.

Verify the downloaded archive with the accompanying `SHA256SUMS`, then extract
it and install the executable:

```sh
sha256sum --check SHA256SUMS
tar -xzf dictator-VERSION-x86_64-linux.tar.gz
mkdir -p ~/.local/bin
install -m 755 dictator ~/.local/bin/dictator
```

Replace `VERSION` with the release version and add `~/.local/bin` to `PATH`.

Dictation needs a running PulseAudio server or PipeWire with `pipewire-pulse`.
On Ubuntu/Debian, install its command-line clients and your desktop tools:

```sh
sudo apt install pulseaudio-utils ca-certificates
# Wayland:
sudo apt install wl-clipboard wtype
# Or X11:
sudo apt install xclip xdotool
```

Run `dictator init`, configure the transcription provider in
`~/.config/dictator/config.json`, then run `dictator daemon`.
Use `dictator toggle` from a second terminal or a desktop shortcut.

For the optional systemd user service, follow the repository README and set
`ExecStart` to the installed executable's absolute path. The supplied source
unit defaults to `~/.cargo/bin/dictator`.
