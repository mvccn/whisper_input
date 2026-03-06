//! Native dialog helpers for configuring the global hotkey from the tray menu.

use std::process::Command;

use anyhow::{Context, Result};
use handy_keys::Hotkey;

use crate::hotkey::{HotkeyBinding, describe_hotkey_binding};

/// Action chosen from the initial hotkey setup dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HotkeySetupAction {
    /// Leave the current binding unchanged.
    Cancel,
    /// Switch back to the default right-command tap.
    UseDefaultCommandTap,
    /// Arm capture mode for a custom key combination.
    CaptureCustomCombo,
}

/// Action chosen after a custom hotkey combination has been captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HotkeyConfirmationAction {
    /// Leave the current binding unchanged.
    Cancel,
    /// Restart capture mode and wait for another combination.
    RetryCapture,
    /// Apply the captured combination as the active binding.
    UseCapturedHotkey,
}

/// Shows the primary hotkey setup dialog.
///
/// # Intent
/// Presents a macOS-native tray workflow for either restoring the default
/// right-command tap or starting custom hotkey capture.
///
/// # Usage
/// Called from the tray event loop when the user selects `Set Hotkey...`.
///
/// # Errors
/// Returns an error if `osascript` cannot be launched successfully.
pub(crate) fn prompt_for_hotkey_setup(
    current_binding: &HotkeyBinding,
) -> Result<HotkeySetupAction> {
    let button = run_dialog(
        "Set WhisperInput Hotkey",
        &build_hotkey_setup_message(current_binding),
        &["Cancel", "Use Default", "Capture Combo"],
        "Capture Combo",
    )?;

    Ok(match button.as_deref() {
        Some("Use Default") => HotkeySetupAction::UseDefaultCommandTap,
        Some("Capture Combo") => HotkeySetupAction::CaptureCustomCombo,
        _ => HotkeySetupAction::Cancel,
    })
}

/// Shows the confirmation dialog after a new combination is captured.
///
/// # Intent
/// Prevents accidental hotkey changes by asking the user to confirm, retry, or
/// cancel after capture completes.
///
/// # Usage
/// Called after the listener reports a captured key combination.
///
/// # Errors
/// Returns an error if `osascript` cannot be launched successfully.
pub(crate) fn prompt_for_hotkey_confirmation(
    current_binding: &HotkeyBinding,
    captured_hotkey: Hotkey,
) -> Result<HotkeyConfirmationAction> {
    let button = run_dialog(
        "Confirm WhisperInput Hotkey",
        &build_hotkey_confirmation_message(current_binding, captured_hotkey),
        &["Cancel", "Retry", "Use Hotkey"],
        "Use Hotkey",
    )?;

    Ok(match button.as_deref() {
        Some("Retry") => HotkeyConfirmationAction::RetryCapture,
        Some("Use Hotkey") => HotkeyConfirmationAction::UseCapturedHotkey,
        _ => HotkeyConfirmationAction::Cancel,
    })
}

/// Builds the user-facing copy for the initial hotkey setup dialog.
fn build_hotkey_setup_message(current_binding: &HotkeyBinding) -> String {
    format!(
        "Current hotkey: {}\n\nChoose how to update the global hotkey.\n\nUse Default resets to Right Command Tap.\nCapture Combo waits for the next shortcut you press. Press Escape to cancel capture.\n\nIf the hotkey still does not respond, use Diagnose Permissions... from the tray menu.",
        describe_hotkey_binding(current_binding)
    )
}

/// Builds the user-facing copy for the hotkey confirmation dialog.
fn build_hotkey_confirmation_message(
    current_binding: &HotkeyBinding,
    captured_hotkey: Hotkey,
) -> String {
    format!(
        "Captured hotkey: {captured_hotkey}\nCurrent hotkey: {}\n\nUse this hotkey for start/stop listening?",
        describe_hotkey_binding(current_binding)
    )
}

/// Runs a simple AppleScript dialog and returns the pressed button label.
///
/// # Intent
/// Keeps the tray app lightweight by reusing macOS-native dialogs instead of
/// creating a dedicated settings window for one short workflow.
///
/// # Usage
/// The dialog text and title are passed as argv to avoid shell-escaping
/// problems with user-visible strings.
///
/// # Errors
/// Returns an error if `osascript` cannot be launched successfully.
fn run_dialog(
    title: &str,
    message: &str,
    buttons: &[&str; 3],
    default_button: &str,
) -> Result<Option<String>> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg("on run argv")
        .arg("-e")
        .arg("set dialogText to item 1 of argv")
        .arg("-e")
        .arg("set dialogTitle to item 2 of argv")
        .arg("-e")
        .arg("set cancelLabel to item 3 of argv")
        .arg("-e")
        .arg("set secondaryLabel to item 4 of argv")
        .arg("-e")
        .arg("set primaryLabel to item 5 of argv")
        .arg("-e")
        .arg("set defaultLabel to item 6 of argv")
        .arg("-e")
        .arg("display dialog dialogText with title dialogTitle buttons {cancelLabel, secondaryLabel, primaryLabel} default button defaultLabel with icon note")
        .arg("-e")
        .arg("button returned of result")
        .arg("-e")
        .arg("end run")
        .arg(message)
        .arg(title)
        .arg(buttons[0])
        .arg(buttons[1])
        .arg(buttons[2])
        .arg(default_button)
        .output()
        .context("failed to launch hotkey dialog")?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(Some(stdout.trim().to_owned()))
}

#[cfg(test)]
mod tests {
    use handy_keys::{Hotkey, Key, Modifiers};

    use super::{build_hotkey_confirmation_message, build_hotkey_setup_message};
    use crate::hotkey::{CommandKeySide, HotkeyBinding};

    #[test]
    fn setup_message_includes_current_binding_and_cancel_hint() {
        let message = build_hotkey_setup_message(&HotkeyBinding::CommandTap(CommandKeySide::Right));
        assert!(message.contains("Current hotkey: Right Command Tap"));
        assert!(message.contains("Use Default"));
        assert!(message.contains("Escape"));
        assert!(message.contains("Diagnose Permissions"));
    }

    #[test]
    fn confirmation_message_includes_current_and_captured_hotkeys() {
        let captured =
            Hotkey::new(Modifiers::CMD | Modifiers::SHIFT, Key::Space).expect("valid hotkey");
        let message = build_hotkey_confirmation_message(
            &HotkeyBinding::CommandTap(CommandKeySide::Right),
            captured,
        );

        assert!(message.contains("Captured hotkey: "));
        assert!(message.contains(&captured.to_string()));
        assert!(message.contains("Current hotkey: Right Command Tap"));
    }
}
