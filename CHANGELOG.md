# Changelog

All notable changes to this project will be documented in this file.

## [0.3.6] - 2026-03-05

### Added
- Added non-blocking lifecycle audio cues using macOS system sounds: `Pop` on listening start and `Glass` when listening stops.

## [0.3.5] - 2026-03-05

### Changed
- Default model preset is now `base` instead of `small` for faster startup and lower transcription latency by default.
- Tray model-size labels now mark `Base` as the default preset.

## [0.3.4] - 2026-03-05

### Added
- Tray `Model Size` submenu with checkable presets (`tiny`, `base`, `small`, `medium`, `large`) so model selection can be changed without restarting.

### Changed
- Worker now accepts model-size change commands and reloads the Whisper runtime after a selection change.
- Model-size changes are blocked while recording is active to avoid interrupting active capture sessions.

## [0.3.3] - 2026-03-05

### Changed
- Enabled `whisper-rs` Metal backend so macOS builds can use GPU acceleration by default.
- Added CLI flags `--no-gpu` and `--no-flash-attn` for runtime fallback/tuning without code changes.

### Performance
- Whisper decode now disables timestamp generation (`no_timestamps`) and uses single-segment mode to reduce end-to-end latency for voice-input use.
- Added decode timing logs (`audio_seconds`, `decode_ms`, `real_time_factor`) to make performance tuning observable.

## [0.3.2] - 2026-03-05

### Fixed
- Moved `Cmd+V` key simulation to the main `tao` event-loop thread to avoid macOS `SIGTRAP` crashes (`dispatch_assert_queue`/`TSMGetInputSourceProperty`) after repeated activations.
- Worker now copies transcript text to clipboard and emits a paste request event instead of simulating keys directly.

## [0.3.1] - 2026-03-05

### Fixed
- Replaced macOS global hotkey backend from `rdev` to `handy-keys` to avoid `SIGTRAP` (`dispatch_assert_queue`) crashes in keyboard event processing.
- Kept left-command tap behavior with anti-chord detection on the new backend.

## [0.3.0] - 2026-03-05

### Added
- Menu bar app runtime (top-right status icon) using `tao` + `tray-icon`.
- Dynamic tray icon state transitions for initializing, idle, listening, processing, and error.
- Tray menu actions for manual start/stop and quit.
- Background worker/event-loop architecture so hotkey capture and transcription continue while app is unfocused.

### Changed
- App now runs as a background accessory app on macOS (Dock icon hidden by activation policy).
- Hotkey workflow now updates tray status in real time while recording/transcribing.

## [0.2.0] - 2026-03-05

### Added
- Automatic Whisper model download and local caching by model size preset.
- New model-size presets: `tiny`, `base`, `small`, `medium`, `large`.
- Default model directory resolution (`~/Library/Caches/whisper_input/models` on macOS).

### Changed
- CLI no longer requires `--model` path.
- CLI now defaults to `--model-size small`.
- Transcription is now fixed to English (`en`) without a language flag.

## [0.1.0] - 2026-03-05

### Added
- Initial macOS-focused Rust utility for local voice input into TUI apps.
- Global left-command tap hotkey listener for start/stop recording.
- Microphone capture pipeline using `cpal`.
- Local Whisper transcription using `whisper-rs`.
- Clipboard copy and optional auto-paste (`Cmd+V`) integration.
- CLI configuration for model path, language, threads, tap window, and recording limit.
- Unit and integration-oriented workflow tests for core state logic.
