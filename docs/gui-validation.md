# Native GUI validation

The GPUI desktop app follows the accepted prototype in `mockups/gui-mockup.html`.
The historical video notes in `mockups/video-feedback.md` describe the prototype
review, including ideas superseded by later decisions.

## Automated checks

Run within the repository development shell:

```sh
direnv exec "$PWD" make check-gui test-gui
direnv exec "$PWD" cargo build --features gui --bins
nix build .#gui
```

The full GUI-feature test suite passed 138 tests, with 3 opt-in probes ignored.
Formatting and all-target Clippy passed with warnings denied.

The GUI workflow integration test uses private temporary XDG directories and a
loopback HTTP provider. It checks failed and successful retries, stable recording
identity and capture time, completion generations, cancellation, measured request
latency, persistence failures, and revision save/conflict/restore behavior. The
CLI compatibility tests run with GUI features enabled as well.

## Desktop acceptance on NixOS and niri

Tested with real GPUI windows at 1120×700 and the tray popup at 340×510:

- History, Stats, and Settings render with bundled IBM Plex Mono and Instrument
  Serif. Recording counts and the latency histogram sit side by side and fit
  within the default window.
- A real microphone capture reached an isolated local transcription provider.
  The new recording became visible and selected on completion, with an empty
  search field. The activity indicator appeared left of search during capture.
- A provider failure stayed in history. Retrying it retained the ID and capture
  timestamp, added an attempt, and did not paste text again.
- Editing created a revision while preserving the original model output. The
  compatibility CLI transcript contained the edited text.
- Playback, pause, and waveform seeking controlled a real `ffplay` process.
- The system tray registered with the session's StatusNotifierWatcher. Repeated
  activation reused the popup. Niri placed it near the activation coordinates.
- Microphone capture delivered PCM before cancellation. This probe used an
  isolated preferences directory. Bluetooth identity/reconnect behavior was
  tested with fixtures; no Bluetooth microphone was connected during validation.

Desktop test audio, history, preferences, and provider configuration were isolated
from the user's data. Clipboard/paste commands from the test daemon were
intercepted. The installed Home Manager daemon was not replaced or activated.

GPUI 0.2.2 creates a normal Wayland toplevel for its popup window kind. On niri,
Dictator uses the compositor's IPC to float, size, and position its own popup.
Other compositors may need a window rule for `Dictator quick controls`.
