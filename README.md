# whisper_input

Local, offline voice-to-text for macOS terminal workflows (Codex, Claude, browser inputs, and other apps).

The app runs as a **menu bar utility** in the top-right status area.

## Intent

Provide fast voice input everywhere by keeping a background hotkey listener and local Whisper transcription, then inserting text at the current cursor location.

## Behavior

- Global hotkey: tap **left Command** to toggle recording.
- First tap: start listening.
- Second tap: stop listening, transcribe, copy, and paste with `Cmd+V` to the currently focused app.
- Audible cues: `Pop` when listening starts and `Glass` when listening stops.
- Tray icon state:
  - gray = idle
  - red = listening
  - blue = processing
  - amber = error

## Current Scope

- macOS-focused background utility with menu bar icon
- global hotkey (left Command tap)
- optional non-interactive one-shot tool mode for app embedding
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

Grant permissions to the built binary (or your terminal app) in System Settings:

1. Privacy & Security -> **Microphone**
2. Privacy & Security -> **Accessibility** (for key simulation / paste)
3. Privacy & Security -> **Input Monitoring** (for global hotkey listening)

If hotkeys or paste do not work, remove/re-add the permission entry and restart the app.

## Quick Start

1. Build:

```bash
cargo build --release
```

2. Run (downloads `base` model automatically if missing):

```bash
cargo run --release
```

3. Use left Command to start/stop voice capture while focused in any input field.

## Embedding Mode

Use one-shot tool mode when embedding `whisper_input` into TUI or GUI apps:

```bash
whisper_input --tool-transcribe-once --tool-record-seconds 6
```

- Records for the requested number of seconds.
- Prints transcript to stdout (empty line when no usable speech was detected).
- Keeps tray/hotkey mode unchanged when `--tool-transcribe-once` is omitted.

You can bias decoding with app context:

```bash
whisper_input \
  --tool-transcribe-once \
  --tool-initial-prompt "Codex, buildkite, memetrader" \
  --tool-initial-prompt-file ~/.cache/whisper_input/context.txt
```

## Tray Menu

- `Start Listening` / `Stop Listening`
- `Model Size` submenu:
  - `Tiny (fastest)`
  - `Base (default)`
  - `Small`
  - `Medium`
  - `Large (slowest)`
- `Quit`

## CLI

```text
Usage: whisper_input [OPTIONS]

Options:
  --model-size <MODEL_SIZE>              Whisper model size preset [possible values: tiny, base, small, medium, large] [default: base]
  --model-dir <MODEL_DIR>                Directory where model binaries are cached [default: ~/Library/Caches/whisper_input/models]
  --threads <THREADS>                    Whisper CPU thread count [default: logical CPU count]
  --max-record-seconds <SECONDS>         Hard cap for one recording [default: 45]
  --hotkey-max-tap-ms <MILLISECONDS>     Maximum press duration for left-command tap [default: 450]
  --no-gpu                               Disable GPU acceleration and force CPU-only inference
  --no-flash-attn                        Disable Flash Attention in Whisper context initialization
  --no-auto-paste                        Skip Cmd+V and only copy transcript to clipboard
  --tool-transcribe-once                 Run one non-interactive capture/transcribe pass and print transcript to stdout
  --tool-record-seconds <SECONDS>        Capture duration in seconds for --tool-transcribe-once [default: 6]
  --tool-initial-prompt <TEXT>           Inline initial prompt text for --tool-transcribe-once
  --tool-initial-prompt-file <PATH>      File containing initial prompt text for --tool-transcribe-once
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
- Choosing a larger model in the tray may trigger a one-time download before that model can be used.
