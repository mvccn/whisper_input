# whisper_input: 
Voice input for any app, designed with fitting in fast agent coding in mind.

Built for `Claude`, `Codex`, `Gemini`, terminals, browsers, and any text field on macOS.

## Why

### Fast: TAP.. SPEAK, faster than you type
- We wanted voice input that keeps coding flow fast and uninterrupted. This can be used anywhere/any app you can type, but the design target is agent coding: short prompts, quick edits, terminal input, and staying in flow without breaking concentration.
- On Apple Silicon Macs, Whisper runs with Metal GPU acceleration. With the `base` or `small` model, results are typically near-instant.

### Free
the project started by using some "open" software and after some free usage, found they charge subscriptions. this one is: 
- 100% free, no free minutes, subscriptions. most of "open" product are not free.
- 100% open-sourced 

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

On first run, macOS will ask for permissions:

- Microphone
- Accessibility
- Input Monitoring

## Use

Default flow:

1. Tap hotkey(default).
2. Speak.
3. Tap hotkey again, and the input should show instantly. It is also copied to clipboard.  
4. try to type and speak at the same time, have fun!

You can change the hotkey and model from `Settings...` in the tray menu.

## Notes

- Default model: `base`
- Also supports: `tiny`, `small`, `medium`, `large`
- The app runs as a macOS menu bar utility
- Listening state shows a red tray animation
- Processing stays visible in the tray while transcription runs

## Future plan: 
 
- tune whiper for complicated code and command
- voice TTS
