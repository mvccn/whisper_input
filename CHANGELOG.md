# Changelog

All notable changes to this project will be documented in this file.

## [0.3.18] - 2026-03-06

### Changed
- Switched the tray's listening animation from the default template waveform to a red pulse, while keeping the other tray states as template icons.
- Rewrote the README into a shorter install-and-use guide focused on local agent-coding voice input, free/open-source positioning, and Apple Silicon performance.

## [0.3.17] - 2026-03-06

### Changed
- Replaced the webview-based `Settings...` window with a native macOS settings window built from standard AppKit controls for hotkey and model changes.
- Restyled the native settings window to use a cleaner macOS-preferences layout with flatter rows, lighter grouping, and less visual chrome.
- Simplified hotkey settings to a single-row layout and made captured key combinations apply immediately instead of requiring an extra confirmation step.
- Moved the `Reset Default` hotkey action onto its own line below the active binding and capture controls for a cleaner layout.

## [0.3.16] - 2026-03-06

### Added
- Added a dedicated `Settings...` window for configuring the active hotkey and Whisper model size without nesting those controls under tray submenus.

### Changed
- Replaced the tray `Hotkey` and `Model Size` submenus with a single `Settings...` entry.
- Moved custom hotkey capture, confirmation, retry, and cancel actions into the settings window.

## [0.3.15] - 2026-03-06

### Changed
- Swapped the recording-start lifecycle cue from `Pop` to `Hero` so the start-listening sound is easier to hear.

## [0.3.14] - 2026-03-06

### Fixed
- macOS installer now re-signs the assembled `WhisperInput.app` bundle with a stable ad-hoc `com.grad.whisper_input` identifier so Accessibility/Input Monitoring permissions stay attached to the installed login item more reliably.
- Startup permission diagnostics no longer treat a just-opened Accessibility prompt as equivalent to a granted permission, avoiding false "passed" results while the login-start hotkey is still unavailable.

## [0.3.13] - 2026-03-06

### Added
- Added a tray `Diagnose Permissions...` action that shows the current macOS permission state for hotkeys, paste, and microphone access, along with the active runtime path.

### Changed
- Simplified the hotkey setup dialog copy and moved permission troubleshooting into the dedicated diagnostics action.

## [0.3.12] - 2026-03-06

### Added
- Added a generated macOS app icon pipeline (`generate_macos_icons.swift`) so installs now include a dedicated `WhisperInput.icns` bundle icon.

### Changed
- Replaced the tray's colored status circles with a waveform-style template icon that better matches macOS menu bar styling.
- Added tray icon animation frames for initializing, listening, and processing states to make recording/transcription activity visible at a glance.

## [0.3.11] - 2026-03-06

### Added
- Added a dialog-driven tray hotkey flow with setup, capture confirmation, retry, and cancel support for custom key combinations.

### Changed
- Hotkey tray UI now exposes `Set Hotkey...` instead of left/right/either command menu items.
- Default command-tap trigger is now right Command across the app and installer.

### Fixed
- Hotkey capture mode now suspends the active trigger until capture completes or is cancelled, preventing accidental recording toggles while choosing a new shortcut.

## [0.3.10] - 2026-03-06

### Added
- Login-start LaunchAgent now exports `WHISPER_EXPECTED_APP_PATH` so startup diagnostics know which installed app bundle should own permissions.

### Changed
- Startup permission diagnostics now log and display both the current permission target path and the expected installed login app path, making bundle-path mismatches visible when hotkeys fail at login.

## [0.3.9] - 2026-03-06

### Fixed
- macOS installer now configures the login LaunchAgent to open `WhisperInput.app` via `/usr/bin/open` instead of starting the inner `Contents/MacOS/whisper_input` binary directly.
- Login-start permission prompts now identify the installed app bundle more reliably in macOS privacy settings.

## [0.3.8] - 2026-03-06

### Added
- Added startup permission diagnostics for macOS Accessibility, Input Monitoring, paste/event-posting access, and microphone authorization.
- Added a startup prompt that can send users to System Settings when required permissions are missing.

### Fixed
- Added single-instance locking so duplicate tray launches do not create multiple running `WhisperInput` copies at login.
- Startup checks now use native macOS permission request APIs first (`Input Monitoring`, synthetic input, microphone) before falling back to a System Settings prompt for anything still unresolved.

## [0.3.7] - 2026-03-06

### Added
- Added `Capture Next Combination...` in the tray `Hotkey` submenu so users can set any key combination at runtime.
- Added tray display of the active binding (`Current: ...`) so the current hotkey is always visible.

### Changed
- Hotkey system now supports two binding modes: command-key tap (`left`/`right`/`either`) and captured custom key combinations.
- Selecting `Left/Right/Either Command` from the tray now explicitly switches binding mode back to command-tap.

## [0.3.6] - 2026-03-05

### Added
- Added non-blocking lifecycle audio cues using macOS system sounds: `Pop` on listening start and `Glass` when listening stops.
- Added `scripts/install_macos_app.sh` to package and install `~/Applications/WhisperInput.app`.
- Installer now configures LaunchAgent auto-start at login (`com.grad.whisper_input`) and writes logs under `~/Library/Logs/whisper_input/`.
- Added `--command-key` hotkey configuration to select `left`, `right`, or `either` command key for recording toggle.
- Installer now accepts `WHISPER_COMMAND_KEY` and wires it into LaunchAgent startup arguments.
- Added `Hotkey Command Key` tray submenu so command-key side can be changed live from the menu bar.

### Fixed
- Hotkey listener now retries initialization every few seconds when permissions are not ready at login, instead of failing once and staying disabled for the rest of the session.

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
