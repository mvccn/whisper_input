//! Configurable command-key tap detection and global listener integration.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use clap::ValueEnum;
use handy_keys::{Hotkey, Key, KeyEvent, KeyboardListener, Modifiers};
use tracing::{error, info, warn};

/// App-level signal emitted by the hotkey listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HotkeyEvent {
    /// Toggle recording state.
    ToggleRecording,
    /// A key combination was captured while hotkey-capture mode was active.
    CapturedKeyCombo(Hotkey),
    /// Hotkey capture mode was cancelled by the user.
    CaptureCancelled,
}

/// Selects which command key side should trigger the hotkey tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CommandKeySide {
    /// Trigger only on left command.
    Left,
    /// Trigger only on right command.
    Right,
    /// Trigger on either command key.
    Either,
}

impl CommandKeySide {
    /// Returns true when the changed modifier matches the configured side.
    fn matches(self, changed: Modifiers) -> bool {
        match self {
            Self::Left => changed == Modifiers::CMD_LEFT,
            Self::Right => changed == Modifiers::CMD_RIGHT,
            Self::Either => changed == Modifiers::CMD_LEFT || changed == Modifiers::CMD_RIGHT,
        }
    }
}

/// Active hotkey binding mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HotkeyBinding {
    /// Trigger on a command-key tap.
    CommandTap(CommandKeySide),
    /// Trigger on a key combination.
    KeyCombo(Hotkey),
}

impl HotkeyBinding {
    /// Returns the default binding used by the tray UI and installer.
    pub(crate) fn default_command_tap() -> Self {
        Self::CommandTap(CommandKeySide::Right)
    }
}

/// Formats the active hotkey binding for tray and dialog display.
pub(crate) fn describe_hotkey_binding(binding: &HotkeyBinding) -> String {
    match binding {
        HotkeyBinding::CommandTap(CommandKeySide::Left) => String::from("Left Command Tap"),
        HotkeyBinding::CommandTap(CommandKeySide::Right) => String::from("Right Command Tap"),
        HotkeyBinding::CommandTap(CommandKeySide::Either) => String::from("Either Command Tap"),
        HotkeyBinding::KeyCombo(hotkey) => format!("Custom ({hotkey})"),
    }
}

/// Runtime control handle for updating hotkey listener settings.
#[derive(Debug, Clone)]
pub(crate) struct HotkeyControl {
    binding: Arc<Mutex<HotkeyBinding>>,
    capture_requested: Arc<AtomicU8>,
}

impl HotkeyControl {
    /// Updates the active listener binding.
    pub(crate) fn set_binding(&self, binding: HotkeyBinding) {
        self.capture_requested.store(0, Ordering::Relaxed);
        *lock_unpoisoned(&self.binding) = binding;
    }

    /// Requests one-shot key-combination capture from the listener thread.
    pub(crate) fn request_capture(&self) {
        self.capture_requested.store(1, Ordering::Relaxed);
    }

    /// Returns the current active hotkey binding.
    pub(crate) fn current_binding(&self) -> HotkeyBinding {
        lock_unpoisoned(&self.binding).clone()
    }
}

/// Detects a short tap of the selected command key without chorded keys.
#[derive(Debug, Clone)]
struct CommandTapDetector {
    command_key_side: CommandKeySide,
    max_tap: Duration,
    command_down: bool,
    chorded: bool,
    pressed_at: Option<Instant>,
}

impl CommandTapDetector {
    /// Creates a detector with the configured tap window.
    fn new(command_key_side: CommandKeySide, max_tap: Duration) -> Self {
        Self {
            command_key_side,
            max_tap,
            command_down: false,
            chorded: false,
            pressed_at: None,
        }
    }

    /// Updates state with a keyboard event and returns a toggle when a tap is confirmed.
    fn on_event(&mut self, event: KeyEvent, now: Instant) -> Option<HotkeyEvent> {
        if let Some(changed) = event.changed_modifier
            && self.command_key_side.matches(changed)
        {
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
pub(crate) fn spawn_listener(
    tx: Sender<HotkeyEvent>,
    initial_binding: HotkeyBinding,
    max_tap: Duration,
) -> HotkeyControl {
    let binding = Arc::new(Mutex::new(initial_binding));
    let thread_binding = binding.clone();
    let capture_requested = Arc::new(AtomicU8::new(0));
    let thread_capture_requested = capture_requested.clone();

    thread::spawn(move || {
        const RETRY_DELAY: Duration = Duration::from_secs(5);

        loop {
            let listener = match KeyboardListener::new() {
                Ok(listener) => listener,
                Err(err) => {
                    error!(
                        error = ?err,
                        retry_seconds = RETRY_DELAY.as_secs(),
                        "failed to initialize keyboard listener; will retry"
                    );
                    thread::sleep(RETRY_DELAY);
                    continue;
                }
            };

            info!("keyboard listener initialized");
            let mut active_binding = lock_unpoisoned(&thread_binding).clone();
            let mut detector = CommandTapDetector::new(CommandKeySide::Left, max_tap);
            let mut combo_armed = true;
            if let HotkeyBinding::CommandTap(side) = active_binding {
                detector = CommandTapDetector::new(side, max_tap);
            }

            while let Ok(event) = listener.recv() {
                if thread_capture_requested.load(Ordering::Relaxed) != 0 {
                    if event.is_key_down && event.key == Some(Key::Escape) {
                        thread_capture_requested.store(0, Ordering::Relaxed);
                        if tx.send(HotkeyEvent::CaptureCancelled).is_err() {
                            warn!("hotkey receiver dropped; listener will stop forwarding events");
                            return;
                        }
                    }

                    if event.is_key_down
                        && let Some(key) = event.key
                    {
                        let normalized_modifiers = normalize_modifiers(event.modifiers);
                        if let Ok(captured) = Hotkey::new(normalized_modifiers, key) {
                            thread_capture_requested.store(0, Ordering::Relaxed);
                            if tx.send(HotkeyEvent::CapturedKeyCombo(captured)).is_err() {
                                warn!(
                                    "hotkey receiver dropped; listener will stop forwarding events"
                                );
                                return;
                            }
                        }
                    }

                    continue;
                }

                let configured_binding = lock_unpoisoned(&thread_binding).clone();
                if configured_binding != active_binding {
                    active_binding = configured_binding;
                    combo_armed = true;
                    if let HotkeyBinding::CommandTap(side) = active_binding {
                        detector = CommandTapDetector::new(side, max_tap);
                        info!(command_key = ?side, "updated hotkey command-key side");
                    } else {
                        info!("updated hotkey key-combination binding");
                    }
                }

                match &active_binding {
                    HotkeyBinding::CommandTap(_) => {
                        if let Some(hotkey_event) = detector.on_event(event, Instant::now())
                            && tx.send(hotkey_event).is_err()
                        {
                            warn!("hotkey receiver dropped; listener will stop forwarding events");
                            return;
                        }
                    }
                    HotkeyBinding::KeyCombo(combo) => {
                        if combo_armed && combo_matches_event(combo, &event) {
                            combo_armed = false;
                            if tx.send(HotkeyEvent::ToggleRecording).is_err() {
                                warn!(
                                    "hotkey receiver dropped; listener will stop forwarding events"
                                );
                                return;
                            }
                            continue;
                        }

                        if !combo_armed
                            && (!combo.modifiers.matches(event.modifiers)
                                || (!event.is_key_down && event.key == combo.key))
                        {
                            combo_armed = true;
                        }
                    }
                }
            }

            warn!(
                retry_seconds = RETRY_DELAY.as_secs(),
                "keyboard listener stopped receiving events; restarting listener"
            );
            thread::sleep(RETRY_DELAY);
        }
    });

    HotkeyControl {
        binding,
        capture_requested,
    }
}

/// Normalizes side-specific modifiers into generic modifier groups.
fn normalize_modifiers(modifiers: Modifiers) -> Modifiers {
    let mut normalized = Modifiers::empty();
    if modifiers.intersects(Modifiers::CMD) {
        normalized |= Modifiers::CMD;
    }
    if modifiers.intersects(Modifiers::SHIFT) {
        normalized |= Modifiers::SHIFT;
    }
    if modifiers.intersects(Modifiers::CTRL) {
        normalized |= Modifiers::CTRL;
    }
    if modifiers.intersects(Modifiers::OPT) {
        normalized |= Modifiers::OPT;
    }
    if modifiers.contains(Modifiers::FN) {
        normalized |= Modifiers::FN;
    }

    normalized
}

/// Returns true when a key event matches a configured key-combination binding.
fn combo_matches_event(combo: &Hotkey, event: &KeyEvent) -> bool {
    event.is_key_down
        && event.key == combo.key
        && combo.key.is_some()
        && combo.modifiers.matches(event.modifiers)
}

/// Acquires a mutex guard even if the mutex was previously poisoned.
fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use handy_keys::{Hotkey, Key, Modifiers};

    use super::{
        CommandKeySide, CommandTapDetector, HotkeyBinding, HotkeyEvent, combo_matches_event,
        describe_hotkey_binding, normalize_modifiers,
    };

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
        let mut detector =
            CommandTapDetector::new(CommandKeySide::Left, Duration::from_millis(450));
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
        let mut detector =
            CommandTapDetector::new(CommandKeySide::Left, Duration::from_millis(100));
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
        let mut detector =
            CommandTapDetector::new(CommandKeySide::Left, Duration::from_millis(450));
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

    #[test]
    fn right_command_tap_triggers_when_configured() {
        let mut detector =
            CommandTapDetector::new(CommandKeySide::Right, Duration::from_millis(450));
        let now = Instant::now();

        assert_eq!(
            detector.on_event(
                make_event(
                    handy_keys::Modifiers::CMD_RIGHT,
                    None,
                    true,
                    Some(handy_keys::Modifiers::CMD_RIGHT)
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
                    Some(handy_keys::Modifiers::CMD_RIGHT)
                ),
                now + Duration::from_millis(80)
            ),
            Some(HotkeyEvent::ToggleRecording)
        );
    }

    #[test]
    fn either_side_ignores_non_matching_config_only_when_required() {
        let mut left_only =
            CommandTapDetector::new(CommandKeySide::Left, Duration::from_millis(450));
        let now = Instant::now();
        left_only.on_event(
            make_event(
                handy_keys::Modifiers::CMD_RIGHT,
                None,
                true,
                Some(handy_keys::Modifiers::CMD_RIGHT),
            ),
            now,
        );
        assert_eq!(
            left_only.on_event(
                make_event(
                    handy_keys::Modifiers::empty(),
                    None,
                    false,
                    Some(handy_keys::Modifiers::CMD_RIGHT)
                ),
                now + Duration::from_millis(80)
            ),
            None
        );

        let mut either =
            CommandTapDetector::new(CommandKeySide::Either, Duration::from_millis(450));
        either.on_event(
            make_event(
                handy_keys::Modifiers::CMD_RIGHT,
                None,
                true,
                Some(handy_keys::Modifiers::CMD_RIGHT),
            ),
            now,
        );
        assert_eq!(
            either.on_event(
                make_event(
                    handy_keys::Modifiers::empty(),
                    None,
                    false,
                    Some(handy_keys::Modifiers::CMD_RIGHT)
                ),
                now + Duration::from_millis(80)
            ),
            Some(HotkeyEvent::ToggleRecording)
        );
    }

    #[test]
    fn normalize_modifiers_merges_command_side_variants() {
        let left = normalize_modifiers(Modifiers::CMD_LEFT);
        let right = normalize_modifiers(Modifiers::CMD_RIGHT);
        assert_eq!(left, Modifiers::CMD);
        assert_eq!(right, Modifiers::CMD);
    }

    #[test]
    fn combo_matching_requires_keydown_and_matching_modifiers() {
        let combo = Hotkey::new(Modifiers::CMD | Modifiers::SHIFT, Key::V).expect("valid hotkey");
        let matching_event =
            make_event(Modifiers::CMD | Modifiers::SHIFT, Some(Key::V), true, None);
        let non_matching_event = make_event(Modifiers::CMD, Some(Key::V), true, None);
        let key_up_event = make_event(Modifiers::CMD | Modifiers::SHIFT, Some(Key::V), false, None);

        assert!(combo_matches_event(&combo, &matching_event));
        assert!(!combo_matches_event(&combo, &non_matching_event));
        assert!(!combo_matches_event(&combo, &key_up_event));
    }

    #[test]
    fn default_command_tap_uses_right_command() {
        assert_eq!(
            HotkeyBinding::default_command_tap(),
            HotkeyBinding::CommandTap(CommandKeySide::Right)
        );
    }

    #[test]
    fn describe_hotkey_binding_formats_command_taps() {
        assert_eq!(
            describe_hotkey_binding(&HotkeyBinding::CommandTap(CommandKeySide::Left)),
            "Left Command Tap"
        );
        assert_eq!(
            describe_hotkey_binding(&HotkeyBinding::CommandTap(CommandKeySide::Right)),
            "Right Command Tap"
        );
        assert_eq!(
            describe_hotkey_binding(&HotkeyBinding::CommandTap(CommandKeySide::Either)),
            "Either Command Tap"
        );
    }

    #[test]
    fn describe_hotkey_binding_formats_custom_combo() {
        let hotkey = Hotkey::new(Modifiers::CMD | Modifiers::SHIFT, Key::V).expect("valid hotkey");
        let summary = describe_hotkey_binding(&HotkeyBinding::KeyCombo(hotkey));
        assert!(summary.starts_with("Custom ("));
        assert!(summary.ends_with(')'));
    }
}
