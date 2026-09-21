# Rust port review

Reviewed `rust-port`, starting at `9ffa662`, against the supplied port report. Six Sol reviewers covered daemon lifecycle, audio, sockets, storage/configuration, CLI/processes, and independent integration review. The lead integrated changes and checked the combined result.

## Findings and changes

| Finding | Consequence | Change |
| --- | --- | --- |
| Daemon, IPC handler, OSD snapshot, and audio owner contained strong reference cycles | Shutdown or dropping an owner could retain tasks, sockets, or audio state | Remove redundant ownership and use weak references where callbacks observe an owner |
| Pipeline read the current cancellation token after spawning; command checks and transitions were separate | Stop/cancel/start could let an older operation affect a new recording | Serialize command transitions and bind pipeline work to an operation identity |
| Recorder timeout stopped capture and discarded its bytes | The daemon remained recording, then failed on stop | Route timeout through the daemon's normal stop/transcribe transition |
| Detached connection/pipeline tasks and unreaped OSD task records | Work could outlive shutdown; reconnect churn retained task records | Track, reap, cancel, and join tasks through shutdown |
| Unbounded IPC frames, connections, and HTTP response bodies | A stalled or oversized peer could consume arbitrary memory | Bound frames/connections and both successful and failed provider responses |
| Cancellation omitted HTTP body reads and clipboard stdin writes | Cancel could hang until a provider or subprocess eventually completed | Cover body reads, pipe writes, and child waits with cancellation; bound subprocess and D-Bus waits |
| Each multipart attempt cloned the whole audio buffer | Upload retries copied the recording on a Tokio worker | Transfer the Vec into shared `Bytes`; retry forms share the allocation |
| Audio callback grew the recording Vec | Allocation pauses occurred during capture | Reserve a validated maximum before opening capture and enforce its sample limit |
| SQLite and recording conversion/file writes ran inside async functions | Busy database/file operations could occupy Tokio workers | Move blocking work to the blocking pool and keep its lifetime tracked |
| IPC startup blindly removed its path | A second daemon could replace a live daemon's socket | Probe without blocking and only remove a confirmed stale socket |
| History sorted only by second-resolution timestamps | The last transcript could be an older row | Use ID as a deterministic descending tiebreaker |
| Invalid stored timestamps became the current time | Corrupt history looked valid | Report the conversion error |
| Second-resolution recording filenames | Rapid recordings could overwrite earlier audio | Add a unique filename suffix and create recordings exclusively |
| Every CLI command created a multithreaded runtime | Version/completion/history commands paid unnecessary startup cost | Construct runtimes only for async commands; use a current-thread runtime for IPC clients |

## Compatibility and bounds

The command names, newline-delimited IPC protocol, OSD event shapes, SQLite schema, and configuration provider layout remain compatible. Audio configuration explicitly requires the format the recorder implements: mono, 16-bit PCM output. Capture is capped at 32 Mi samples, or 128 MiB of float samples, which permits about 34 minutes at 16 kHz. Invalid or excessive capture sizes fail before opening the device. Recording filenames gain a UUID suffix. A new recording is rejected while a previous cancelled pipeline is still draining, so two operations cannot compete for the recorder.

Successful transcription responses are limited to 1 MiB; error responses to 8 KiB. IPC requests and responses are limited to 64 KiB, with at most 64 active server connections. OSD retains its four-client limit, 16-state-event queue, and latest-only meter delivery. Clipboard helpers have bounded execution time. These limits protect process resources and can reject previously accepted excessive inputs.

## Verification

The original 17 tests passed before changes. A new stalled-response cancellation test was also run against an isolated copy of `9ffa662`; it failed at the 500 ms cancellation deadline. The revised implementation passes that test for both success and error responses.

The final source passes `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-targets --all-features`: 58 tests passed. Two internal subprocess helper tests are marked ignored for direct execution; their parent tests launch them explicitly with isolated XDG environments.

The tests include actual daemon scheduling/lifetime fixtures: an old cancelled pipeline cannot mutate a replacement operation, timeout and manual stop can claim a recording only once, 10,000 notification updates keep one worker, root shutdown cancels the operation, and shutdown releases the daemon's weak reference. A further current-thread runtime test reproduced shutdown starving an unrelated timer while waiting on the command mutex; moving that wait to the blocking pool makes the test pass. Synthetic recorder tests cover bounded capture, cancellation, poisoned-lock drop cleanup, and coalesced meter delivery.

The final independent integration review found no remaining must-fix issues. The Nix release package built successfully and passed its release-mode tests. Its binary reports version `2.4.0`; Bash, Zsh, and Fish completion files are installed, and Bash completion syntax passes `bash -n`. The build used an isolated source snapshot; every compiled source, Cargo manifest/lock, and flake file was compared byte-for-byte with the working tree.

### Local performance measurements

A release-mode socket probe used 16 concurrent IPC clients, three rounds of 10,000 status requests, and 1,000 OSD reconnects per round. All 30,000 requests and 3,000 reconnects completed. Each measured round retained 12 descriptors and 17 threads. RSS was 4,284, 4,196, and 4,196 KiB across the rounds; shutdown removed both socket paths and reduced descriptors to 10. Threads belong to Tokio's reusable blocking pool and remain until its idle timeout or runtime shutdown. This finite run found no growth proportional to reconnect count; it is not a proof against every leak.

The rounds took 258–265 ms for 10,000 requests, with median request latency of 394–406 microseconds and p95 of 517–562 microseconds. Run `cargo run --release --example stress_ipc` to repeat the private-socket probe.

A standalone synthetic encoding harness used the old and revised conversion loops, 4.8 million float samples, Rust `-C opt-level=3 -C codegen-units=1`, and 30 alternating runs per repeat. Both generated identical 9,600,044-byte WAV output. Across three repeats, the median was 4,875 microseconds for the old two-stage path and 4,392 for direct WAV encoding, about 9.9% lower. Post-capture allocation traffic fell from 19,200,044 bytes in two allocations to 9,600,044 bytes in one. Theoretical peak live audio storage remains about 28.8 MB; this reduces allocation traffic, not the peak.

A 100-run interleaved `version` probe with private XDG directories measured debug-build median startup at 4.12 ms before and 3.34 ms after removing unnecessary runtime startup. The revised release build measured 2.39 ms. Background compilation was running, so these numbers are illustrative local measurements, not a controlled cross-machine benchmark.

Tests use private Unix sockets, local mock HTTP servers, synthetic audio, temporary databases, and explicit subprocess stubs. They do not send dictated text to an external provider or paste into the user's desktop.

## Remaining runtime checks

A hardware backend can block inside an operating-system or audio-library call; Rust cannot safely kill an arbitrary stuck thread. Moving those calls off Tokio workers protects async scheduling but does not prove a hard shutdown deadline for a wedged driver.

Real microphone capture/device removal, desktop clipboard delivery, and the configured transcription provider need a live acceptance pass. The review does not activate a Home Manager service or replace the running daemon. Audio files, transcript history, and logs remain persistent by design; this change does not add a retention policy.

## Live Electron paste follow-up

A desktop test reproduced duplicate insertion in T3 Code with one synthetic Ctrl+Shift+V, independently of the daemon. Changing clipboard-helper builds did not resolve it. Synthetic Ctrl+V inserted the probe once. The dedicated XF86Paste key did nothing in T3 Code; Shift+Insert was not adopted because Ghostty binds it to the primary selection rather than the regular clipboard. These observations isolate a shortcut compatibility problem but do not explain why the user first observed it after switching from Go.

Added an optional default paste shortcut and exact Niri application overrides. The live test configuration selects Ctrl+V for `com.t3tools.T3Code`, retaining Ctrl+Shift+V elsewhere. Niri focus queries use one read-only IPC connection, a 16 KiB response limit, cancellation, and a 250 ms deadline. Missing or unavailable focus information falls back to the configured default. Other compositors and X11 retain the default shortcut.

After this follow-up, 64 tests pass, including legacy configuration compatibility, override selection, malformed/oversized focus replies, query timeout and cancellation. Clippy passes with warnings denied. The manual shortcut test verified single insertion in T3 Code. The updated release daemon was then launched in the `stt` tmux session with a private runtime configuration containing the T3 Code override, and the user confirmed dictation works. The Home Manager configuration remains unchanged.
