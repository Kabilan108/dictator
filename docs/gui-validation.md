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

The full GUI-feature test suite passed 187 tests, with 6 helper/probe entries ignored by default.
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

- Dictation, History, Stats, and Settings render with bundled IBM Plex Mono and
  Geist. Dictation is the default tab. Stats fits within the default
  window: five summary tiles, 30-day activity and time-of-day charts, then the
  latency histogram beside recording-length figures and the model table.
- Dictation: Start recording drove the live daemon through the same toggle IPC
  as the shortcut; the meter and elapsed time updated during capture and the
  finished transcript appeared under Last result and at the top of Recent
  dictations. Check connection reported the real provider host as reachable
  (HTTP 405 without credentials) with the measured round trip.
- Deleting a recording from History after the inline confirmation removed the
  row, its revision and attempt rows, the legacy compatibility transcript, and
  the WAV file; the list re-selected the next recording.
- A real microphone capture reached an isolated local transcription provider.
  The new recording became visible and selected on completion, with an empty
  search field. The activity indicator appeared left of search during capture.
- A provider failure stayed in history. Retrying it retained the ID and capture
  timestamp, added an attempt, and did not paste text again.
- Editing created a revision while preserving the original model output. The
  compatibility CLI transcript contained the edited text. The detail pane shows
  one diff, the model transcript against the current text, and only when they
  differ; the revision list and restore action were removed as unhelpful.
- Playback, pause, and waveform seeking controlled a real `ffplay` process.
- The system tray registered with the session's StatusNotifierWatcher. Repeated
  activation reused the popup. Niri placed it near the activation coordinates.
- Microphone capture delivered PCM before cancellation. This probe used an
  isolated preferences directory. Bluetooth identity/reconnect behavior was
  tested with fixtures; no Bluetooth microphone was connected during validation.

Desktop test audio, history, preferences, and provider configuration were isolated
from the user's data. Clipboard/paste commands from the test daemon were
intercepted. The installed Home Manager daemon was not replaced or activated.

GPUI 0.2.2 creates a normal Wayland toplevel for its popup window kind and does
not request an initial size for any window. On niri, Dictator uses the
compositor's IPC to float, size, and position its own popup, and to size the
main window to 924×740 after it opens. Other compositors may need window rules
for `Dictator` and `Dictator quick controls`.

GPUI's Linux backend stops its event loop when the last window closes. With the
tray enabled, closing the main window therefore replaces the process with a
tray-only instance (`dictator-gui --tray`); the tray item re-registers and can
reopen the window. Only the tray's Quit item ends the process.

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
