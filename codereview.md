# Code Review

Date: 2026-06-05

## Findings

### `src-tauri/resources/stt-server.py:505` and `src-tauri/src/voice/sherpa_wake.rs:239`
Impact: The wake path never verifies that a wake phrase was spoken. `/wake/check` only runs VAD plus speaker matching, and the Rust wake worker triggers as soon as `speaker_match` is true.

Suggested fix: Add real wake-phrase detection before emitting the wake event, instead of treating any speech from a matched speaker as a valid wake.

### `src-tauri/src/voice/sherpa_wake.rs:93`
Impact: Wake startup hardcodes `Me.bin`, but enrollment saves `{name}.bin`. Because the settings flow prompts arbitrary names, a successful enrollment will usually still leave wake startup failing with `No enrolled voiceprint`.

Suggested fix: Align wake lookup with the enrollment naming scheme, or persist and load the selected enrolled speaker explicitly.

### `src-tauri/src/voice/sherpa_wake.rs:118`
Impact: Wake listening always requires an enrolled voiceprint, even when `speaker_verification_enabled` is off. That breaks the documented wake-only mode where phrase detection should work without speaker verification.

Suggested fix: Gate speaker verification separately from wake activation so wake-only mode can start without a stored voiceprint.

### `src-tauri/src/voice/sherpa_wake.rs:110` and `src-tauri/resources/stt-server.py:572`
Impact: The wake threshold UI setting is currently ineffective. `start_wake_listener(app, _threshold)` discards the configured value, and the server uses a hardcoded similarity threshold, so changing the slider does not change runtime behavior.

Suggested fix: Thread the configured threshold through the Rust and Python wake path, or remove/disable the slider until it is implemented.

## Checks Run

- `cargo check` in `src-tauri/` passed.
- `vite build` passed.
- `python3 -m py_compile src-tauri/resources/stt-server.py` passed.
- No frontend `check` script exists in `package.json`.
