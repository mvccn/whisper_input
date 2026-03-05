//! Clipboard and optional paste helpers.

use anyhow::{Result, anyhow};
use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

/// Copies text to clipboard.
///
/// # Errors
/// Returns an error when clipboard access fails.
pub(crate) fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut clipboard = Clipboard::new().map_err(|err| anyhow!("clipboard unavailable: {err}"))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|err| anyhow!("failed to set clipboard text: {err}"))?;
    Ok(())
}

/// Emits Command+V on macOS.
///
/// # Errors
/// Returns an error when synthetic key events cannot be created.
pub(crate) fn paste_cmd_v() -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|err| anyhow!("failed to initialize input simulator: {err}"))?;

    enigo
        .key(Key::Meta, Direction::Press)
        .map_err(|err| anyhow!("failed to press command key: {err}"))?;
    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|err| anyhow!("failed to click v key: {err}"))?;
    enigo
        .key(Key::Meta, Direction::Release)
        .map_err(|err| anyhow!("failed to release command key: {err}"))?;

    Ok(())
}
