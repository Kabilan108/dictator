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

The full GUI-feature test suite passed 169 tests, with 6 helper/probe entries ignored by default.
Formatting and all-target Clippy passed with warnings denied.
The Nix GUI package also passed its release-profile tests and launched on this
machine without a development shell or an inherited `LD_LIBRARY_PATH`.

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
  compatibility CLI transcript contained the edited text. Restoring revision 0
  updated the native editor immediately and appended a restore revision.
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

Review follow-up checks covered unavailable desktop notifications, stalled Pulse
and Niri subprocesses, deferred editor/search events, legacy failure discovery,
and cancellation during capture-failure persistence. A temporary systemd user
unit could write the exact Dictator app directories while an unrelated home-file
write was denied. The retry workflow passed 50 consecutive focused runs after
one unreproduced timeout, followed by a successful full suite.

A final native lifecycle check reopened the tray five times and then restarted the
isolated daemon while the main window remained open. History, statistics, and
recording controls stayed available. A subprocess regression and a negative-control
probe verified that opening another database connection preserves process locks.
