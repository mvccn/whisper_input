//! Left-command tap detection and global listener integration.

use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use handy_keys::{KeyEvent, KeyboardListener, Modifiers};
use tracing::{error, warn};

/// App-level signal emitted by the hotkey listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HotkeyEvent {
    /// Toggle recording state.
    ToggleRecording,
}

/// Detects a short tap of the left command key without chorded keys.
#[derive(Debug, Clone)]
struct LeftCommandTapDetector {
    max_tap: Duration,
    command_down: bool,
    chorded: bool,
    pressed_at: Option<Instant>,
}

impl LeftCommandTapDetector {
    /// Creates a detector with the configured tap window.
    fn new(max_tap: Duration) -> Self {
        Self {
            max_tap,
            command_down: false,
            chorded: false,
            pressed_at: None,
        }
    }

    /// Updates state with a keyboard event and returns a toggle when a tap is confirmed.
    fn on_event(&mut self, event: KeyEvent, now: Instant) -> Option<HotkeyEvent> {
        if event.changed_modifier == Some(Modifiers::CMD_LEFT) {
            if event.is_key_down {
                if !self.command_down {
                    self.command_down = true;
                    self.chorded = false;
                    self.pressed_at = Some(now);
                }
                return None;
            }

            let is_tap = self.command_down
                && !self.chorded
                && self
                    .pressed_at
                    .map(|pressed| now.duration_since(pressed) <= self.max_tap)
                    .unwrap_or(false);

            self.command_down = false;
            self.chorded = false;
            self.pressed_at = None;

            if is_tap {
                return Some(HotkeyEvent::ToggleRecording);
            }

            return None;
        }

        if self.command_down && (event.key.is_some() || event.changed_modifier.is_some()) {
            self.chorded = true;
        }

        None
    }
}

/// Spawns the global event listener and forwards hotkey events to the app channel.
pub(crate) fn spawn_listener(tx: Sender<HotkeyEvent>, max_tap: Duration) {
    thread::spawn(move || {
        let listener = match KeyboardListener::new() {
            Ok(listener) => listener,
            Err(err) => {
                error!(error = ?err, "failed to initialize keyboard listener");
                return;
            }
        };

        let mut detector = LeftCommandTapDetector::new(max_tap);

        while let Ok(event) = listener.recv() {
            if let Some(hotkey) = detector.on_event(event, Instant::now())
                && tx.send(hotkey).is_err()
            {
                warn!("hotkey receiver dropped; listener will stop forwarding events");
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use handy_keys::Key;

    use super::{HotkeyEvent, LeftCommandTapDetector};

    fn make_event(
        modifiers: handy_keys::Modifiers,
        key: Option<Key>,
        is_key_down: bool,
        changed_modifier: Option<handy_keys::Modifiers>,
    ) -> handy_keys::KeyEvent {
        handy_keys::KeyEvent {
            modifiers,
            key,
            is_key_down,
            changed_modifier,
        }
    }

    #[test]
    fn quick_left_command_tap_triggers_toggle() {
        let mut detector = LeftCommandTapDetector::new(Duration::from_millis(450));
        let now = Instant::now();

        assert_eq!(
            detector.on_event(
                make_event(
                    handy_keys::Modifiers::CMD_LEFT,
                    None,
                    true,
                    Some(handy_keys::Modifiers::CMD_LEFT)
                ),
                now
            ),
            None
        );

        assert_eq!(
            detector.on_event(
                make_event(
                    handy_keys::Modifiers::empty(),
                    None,
                    false,
                    Some(handy_keys::Modifiers::CMD_LEFT)
                ),
                now + Duration::from_millis(80)
            ),
            Some(HotkeyEvent::ToggleRecording)
        );
    }

    #[test]
    fn long_hold_does_not_trigger_toggle() {
        let mut detector = LeftCommandTapDetector::new(Duration::from_millis(100));
        let now = Instant::now();

        detector.on_event(
            make_event(
                handy_keys::Modifiers::CMD_LEFT,
                None,
                true,
                Some(handy_keys::Modifiers::CMD_LEFT),
            ),
            now,
        );

        assert_eq!(
            detector.on_event(
                make_event(
                    handy_keys::Modifiers::empty(),
                    None,
                    false,
                    Some(handy_keys::Modifiers::CMD_LEFT)
                ),
                now + Duration::from_millis(120)
            ),
            None
        );
    }

    #[test]
    fn chorded_command_does_not_trigger_toggle() {
        let mut detector = LeftCommandTapDetector::new(Duration::from_millis(450));
        let now = Instant::now();

        detector.on_event(
            make_event(
                handy_keys::Modifiers::CMD_LEFT,
                None,
                true,
                Some(handy_keys::Modifiers::CMD_LEFT),
            ),
            now,
        );

        detector.on_event(
            make_event(handy_keys::Modifiers::CMD_LEFT, Some(Key::V), true, None),
            now + Duration::from_millis(10),
        );

        assert_eq!(
            detector.on_event(
                make_event(
                    handy_keys::Modifiers::empty(),
                    None,
                    false,
                    Some(handy_keys::Modifiers::CMD_LEFT)
                ),
                now + Duration::from_millis(80)
            ),
            None
        );
    }
}
