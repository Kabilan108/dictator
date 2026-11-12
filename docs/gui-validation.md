# Desktop app validation

The desktop app is drawn with [Rune](https://github.com/anomalyco/rune). It
replaced the earlier GPUI app and keeps that app's backend worker, playback and
level feed. `mockups/gui-mockup.html` and `mockups/video-feedback.md` describe
the prototype the first version followed; they are history, not a spec.

## Automated checks

Run within the repository development shell:

```sh
direnv exec "$PWD" make check-gui test-gui
direnv exec "$PWD" cargo build --features gui --bins
nix build .#gui
```

The GUI workflow integration test uses private temporary XDG directories and a
loopback HTTP provider. It checks failed and successful retries, stable recording
identity and capture time, completion generations, cancellation, measured request
latency, persistence failures, and revision save/conflict/restore behavior.

View-independent logic (reply bookkeeping, drafts, the word diff, formatting,
histogram bins) is unit-tested in `src/gui/logic.rs` and `src/gui/charts.rs`.

## Looking at a change

Every pane has fixture scenes that render headlessly, with no window, from the
demo backend:

```sh
dictator-gui --demo --list-scenes
dictator-gui --demo --snap out.png --scene settings --theme light --scale 2
dictator-gui --demo --snap out.png --scene settings --size 940x1120
```

Scenes: `dictation`, `recording`, `history`, `history-edited`, `history-delete`, `history-failed`,
`stats`, `settings`, `settings-editing`. A scene is a fixture, not end-to-end
proof: the real window, input and a live daemon still need a person.

## Checked for the Rune rebuild (NixOS, niri)

- Each scene above rendered in the dark and light themes and was inspected.
- The real window, started with `--demo`, opened floating at 940×700. niri
  reported a single open event at that size and no later resize, with the
  user's old `org.dictator.Dictator` window rule (924×740) still in place. The
  window maps at a fixed size, which tiling compositors float; it becomes
  resizable after its first frame. No window rule is needed.
- The app starts on Dictation.

## Not rechecked

These were accepted on the GPUI app and exercise code the rebuild did not
change (the backend worker, `playback.rs`, the daemon), but nobody has repeated
them through the new views:

- recording through the window against a live daemon, and the meter following
  the OSD socket's level stream;
- playback, pause and waveform seeking against a real `ffplay` process;
- saving an edit, retrying a failed recording, and deleting one, on real data;
- switching provider, forgetting a microphone and renaming a glossary term
  against a live daemon (covered by unit and IPC tests and by the demo backend);
- a daemon restart while the window stays open.

Closing the window ends the process. The GUI is opened on demand from the
desktop shell's dictation panel, which also owns the bar indicator and quick
recording controls.
