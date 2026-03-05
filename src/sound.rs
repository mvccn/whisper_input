//! Playback helpers for recording lifecycle status sounds.

use std::process::Command;

use tracing::warn;

/// Distinct sound cues for recording state transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SoundCue {
    ListeningStarted,
    ListeningStopped,
}

/// Plays a status cue asynchronously on macOS.
///
/// Errors are logged and never propagated to avoid interrupting recording.
pub(crate) fn play(cue: SoundCue) {
    let sound_path = system_sound_path(cue);
    if let Err(err) = Command::new("afplay").arg(sound_path).spawn() {
        warn!(
            error = %err,
            ?cue,
            sound_path,
            "failed to play lifecycle sound"
        );
    }
}

/// Maps a cue to a built-in macOS system sound asset.
fn system_sound_path(cue: SoundCue) -> &'static str {
    match cue {
        SoundCue::ListeningStarted => "/System/Library/Sounds/Pop.aiff",
        SoundCue::ListeningStopped => "/System/Library/Sounds/Glass.aiff",
    }
}

#[cfg(test)]
mod tests {
    use super::{SoundCue, system_sound_path};

    #[test]
    fn listening_started_has_distinct_sound() {
        assert_eq!(
            system_sound_path(SoundCue::ListeningStarted),
            "/System/Library/Sounds/Pop.aiff"
        );
    }

    #[test]
    fn listening_stopped_has_distinct_sound() {
        assert_eq!(
            system_sound_path(SoundCue::ListeningStopped),
            "/System/Library/Sounds/Glass.aiff"
        );
    }
}
