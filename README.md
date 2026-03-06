# whisper_input

Voice input for any app, designed with agent coding in mind.

Built for `Claude`, `Codex`, `Gemini`, terminals, browsers, and any text field on macOS.

## Why

We wanted voice input that keeps coding flow fast and uninterrupted.

This can be used anywhere you can type, but the design target is agent coding: short prompts, quick edits, terminal input, and staying in flow without breaking concentration.

- No subscription.
- No usage meter.
- No limited free minutes.
- True open source.
- Works offline after the model is downloaded.

Products like OpenWhisper and SuperWhisper are polished, but they are subscription products. This project is simpler: install it, run it, talk, get text.

On Apple Silicon Macs, Whisper runs with Metal GPU acceleration. With the `base` or `small` model, results are typically near-instant.

## Install

Requirements:

- macOS
- Rust stable

Build and run:

```bash
cargo run --release
```

Install as a menu bar app:

```bash
./scripts/install_macos_app.sh
```

On first run, macOS will ask for:

- Microphone
- Accessibility
- Input Monitoring

## Use

Default flow:

1. Tap Right Command once to start listening.
2. Speak.
3. Tap Right Command again to stop.
4. The app transcribes locally, copies the text, and pastes it into the current app.

You can change the hotkey and model from `Settings...` in the tray menu.

## Notes

- Default model: `base`
- Also supports: `tiny`, `small`, `medium`, `large`
- The app runs as a macOS menu bar utility
- Listening state shows a red tray animation
- Processing stays visible in the tray while transcription runs

## Project Goal

Make voice input fit naturally into a fast, non-interrupting coding workflow, especially for agent-driven work.
