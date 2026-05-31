# Dictator QuickShell OSD

Reference QuickShell client for Dictator's visual OSD socket.

```bash
quickshell --path examples/quickshell-osd
```

The client connects to `$XDG_RUNTIME_DIR/dictator/osd.sock`, or `/tmp/dictator-osd-$USER/osd.sock` when `XDG_RUNTIME_DIR` is unavailable. It expects Dictator to own the socket and emit newline-delimited JSON state and meter events.

For visual-only development without Dictator running:

```bash
DICTATOR_OSD_DEMO=1 quickshell --path examples/quickshell-osd
```

To preview the transcribing animation directly:

```bash
DICTATOR_OSD_DEMO=1 DICTATOR_OSD_DEMO_STATE=transcribing quickshell --path examples/quickshell-osd
```

For socket/event debugging:

```bash
DICTATOR_OSD_DEBUG=1 quickshell --path examples/quickshell-osd
```

The implementation keeps the compact waveform pill from the original prototype, but uses Dictator's production protocol:

```json
{"type":"state","value":"recording","recording_duration_ms":0}
{"type":"meter","rms":0.03,"peak":0.2}
{"type":"state","value":"transcribing","recording_duration_ms":4820}
{"type":"state","value":"typing"}
{"type":"state","value":"error","message":"transcription failed"}
{"type":"state","value":"idle"}
```

Connection behavior:

- Retries are capped at 30 attempts per outage.
- Retry delay starts at 1 second and backs off to 8 seconds.
- The OSD stays hidden while disconnected.
- Invalid events are ignored.
- Meter events are accepted only while the current state is `recording`.
- `DICTATOR_OSD_DEMO_STATE` accepts `recording`, `transcribing`, `typing`, or `error`.
