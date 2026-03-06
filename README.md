# whisper_input

Local, offline voice-to-text for macOS terminal workflows (Codex, Claude, browser inputs, and other apps).

The app runs as a **menu bar utility** in the top-right status area.

## Intent

Provide fast voice input everywhere by keeping a background hotkey listener and local Whisper transcription, then inserting text at the current cursor location.

## Behavior

- Global hotkey and Whisper model size are configurable from a dedicated native macOS `Settings...` window opened from the tray menu.
- Default trigger is a **Right Command** tap.
- You can capture any key combination (for example `Cmd+Shift+Space`) from the settings window, then confirm it before applying.
- The tray menu includes `Diagnose Permissions...` so you can re-check hotkey, paste, and microphone access on demand.
- On startup, the app checks for required macOS permissions and now asks macOS to show native permission prompts where possible.
- Only one `WhisperInput` instance is allowed to run at a time; duplicate launches exit immediately.
- First tap: start listening.
- Second tap: stop listening, transcribe, copy, and paste with `Cmd+V` to the currently focused app.
- Audible cues: `Hero` when listening starts and `Glass` when listening stops.
- Tray icon state:
  - waveform mark = idle
  - red animated pulse = listening
  - animated left-to-right sweep = processing
  - compact warning waveform = error

## Current Scope

- macOS-focused background utility with menu bar icon
- global hotkey (default right-command tap, or a confirmed custom key combination)
- microphone capture from default input device
- local Whisper inference via `whisper-rs` (`whisper.cpp` backend)
- macOS Metal GPU acceleration enabled by default (`use_gpu=1`)
- automatic model download/cache by size preset
- English transcription mode (`en`)
- clipboard + auto-paste into focused application

## Requirements

- macOS (Apple Silicon or Intel)
- Rust toolchain (`rustup`, stable)
- Internet access on first run to download a model

## macOS Permissions

Grant permissions to the entry macOS is actually launching in System Settings:

- `WhisperInput.app` for the installed menu-bar app
- your terminal app for `cargo run --release`

1. Privacy & Security -> **Microphone**
2. Privacy & Security -> **Accessibility** (for key simulation / paste)
3. Privacy & Security -> **Input Monitoring** (for global hotkey listening)

If hotkeys or paste do not work, remove/re-add the permission entry and restart the app.

At login-start, `WhisperInput` now first asks macOS to show native permission prompts where possible. If something still needs manual approval, it then offers to open the right System Settings page.

You can also use `Diagnose Permissions...` from the tray menu at any time to inspect the current runtime path, see which permissions are missing, and jump straight to System Settings.

The installed login item now launches `WhisperInput.app` itself instead of the inner `Contents/MacOS/whisper_input` binary, which makes macOS permission UI associate the entry with the app bundle more reliably.

On login-start, startup diagnostics now log and display the exact permission target path plus the expected installed app path from the LaunchAgent, so you can tell whether macOS is evaluating permissions for `~/Applications/WhisperInput.app` or for some other runtime path.

## Quick Start

1. Build:

```bash
cargo build --release
```

2. Run (downloads `base` model automatically if missing):

```bash
cargo run --release
```

3. Use your configured hotkey (command tap or captured combo) to start/stop voice capture while focused in any input field.

## Install As App (macOS)

Build and install as a menu-bar app in `~/Applications`, then configure auto-start at login:

```bash
./scripts/install_macos_app.sh
```

Use a specific command-tap side for the installed auto-start app:

```bash
WHISPER_COMMAND_KEY=right ./scripts/install_macos_app.sh
```

Installed artifacts:

- App bundle: `~/Applications/WhisperInput.app`
- App icon: generated `WhisperInput.icns` inside `~/Applications/WhisperInput.app/Contents/Resources/`
- LaunchAgent: `~/Library/LaunchAgents/com.grad.whisper_input.plist`
- LaunchAgent logs: `~/Library/Logs/whisper_input/`

The installer now re-signs the assembled app bundle with a stable ad-hoc `com.grad.whisper_input` identity so macOS privacy permissions are more likely to remain attached to the login-start app.

If you installed an older build and the hotkey still fails only at login, rerun `./scripts/install_macos_app.sh`, then remove and re-add `WhisperInput.app` once in:

1. Privacy & Security -> **Accessibility**
2. Privacy & Security -> **Input Monitoring**

## Tray Menu

- `Start Listening` / `Stop Listening`
- `Settings...` (opens a native macOS window for hotkey and model-size changes)
- `Diagnose Permissions...` (shows current permission status for the running app path and opens System Settings when needed)
- `Quit`

## Settings Window

- Built from standard macOS controls (`text`, `buttons`, and a native model picker) in a cleaner macOS-preferences style layout instead of embedded HTML/CSS content.
- Hotkey section:
  - keeps the active binding and capture button on the main row, with the reset button directly below
  - captures a custom key combination and applies it immediately
  - resets to the default right-Command tap
- Model section:
  - selects `Tiny`, `Base`, `Small`, `Medium`, or `Large`
  - applies model changes immediately when recording is idle

## CLI

```text
Usage: whisper_input [OPTIONS]

Options:
  --model-size <MODEL_SIZE>              Whisper model size preset [possible values: tiny, base, small, medium, large] [default: base]
  --model-dir <MODEL_DIR>                Directory where model binaries are cached [default: ~/Library/Caches/whisper_input/models]
  --threads <THREADS>                    Whisper CPU thread count [default: logical CPU count]
  --max-record-seconds <SECONDS>         Hard cap for one recording [default: 45]
  --command-key <COMMAND_KEY>            Command key side used for command-tap hotkey mode [possible values: left, right, either] [default: right]
  --hotkey-max-tap-ms <MILLISECONDS>     Maximum press duration for command-key tap [default: 450]
  --no-gpu                               Disable GPU acceleration and force CPU-only inference
  --no-flash-attn                        Disable Flash Attention in Whisper context initialization
  --no-auto-paste                        Skip Cmd+V and only copy transcript to clipboard
  -h, --help                             Print help
```

## Model Notes

- Default preset is `base` (`ggml-base.en.bin`).
- `tiny`, `base`, `small`, and `medium` use `.en` English models.
- `large` maps to `ggml-large-v3.bin` because there is no `.en` large asset in whisper.cpp model files.
- Models are downloaded from `huggingface.co/ggerganov/whisper.cpp` and cached locally.

## Notes

- Audio is converted to mono and resampled to 16 kHz for Whisper compatibility.
- The app uses the default input device; set your preferred mic in macOS Sound settings.
- Inference now uses Metal GPU by default; use `--no-gpu` to fall back to CPU.
- Auto-paste (`Cmd+V`) is executed on the main menu-bar event loop thread for macOS input-system stability.
- Choosing a larger model from the settings window may trigger a one-time download before that model can be used.
- Captured custom hotkeys apply immediately for the running app session after you confirm them in the settings window.
- During hotkey capture, press `Escape` to cancel without changing the current binding.
- Duplicate launches are suppressed with a single-instance lock so login-start and manual launch do not create two tray icons.
- The tray icon keeps the idle, processing, and error states as template waveforms, while the listening animation switches to a red pulse.
