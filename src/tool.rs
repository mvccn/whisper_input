//! Non-interactive tool entry points for embedding in other applications.

use std::thread;
use std::time::Duration;

use anyhow::Result;
use tracing::info;

use crate::audio::{Recorder, has_minimum_signal, to_whisper_samples};
use crate::config::{Cli, Config};
use crate::model;
use crate::transcribe::WhisperEngine;

const MAX_INITIAL_PROMPT_CHARS: usize = 512;

/// Runs one capture/transcribe pass and writes the transcript to stdout.
///
/// # Errors
/// Returns an error when model loading, microphone setup, or decoding fails.
pub(crate) fn run_transcribe_once(config: &Config, cli: &Cli) -> Result<()> {
    let model_path = model::ensure_model(config.model_size, &config.model_dir)?;
    let recorder = Recorder::new(cli.tool_record_seconds)?;
    let whisper = WhisperEngine::new(
        &model_path,
        config.threads,
        config.use_gpu,
        config.flash_attn,
    )?;
    let prompt = cli
        .resolve_tool_initial_prompt()?
        .map(|text| truncate_initial_prompt(&text));

    info!(
        record_seconds = cli.tool_record_seconds,
        prompt_chars = prompt.as_ref().map_or(0, |text| text.chars().count()),
        "starting one-shot transcription"
    );

    let recording = recorder.start()?;
    thread::sleep(Duration::from_secs(cli.tool_record_seconds));
    let captured = recording.stop();
    let whisper_samples = to_whisper_samples(&captured);

    if !has_minimum_signal(&whisper_samples) {
        info!("captured audio did not meet minimum signal threshold");
        println!();
        return Ok(());
    }

    let transcript = whisper.transcribe_with_prompt(&whisper_samples, prompt.as_deref())?;
    println!("{transcript}");
    Ok(())
}

/// Truncates prompt text so decode bias stays focused and bounded.
fn truncate_initial_prompt(prompt: &str) -> String {
    prompt.chars().take(MAX_INITIAL_PROMPT_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::truncate_initial_prompt;

    #[test]
    fn initial_prompt_is_truncated_to_reasonable_size() {
        let input = "x".repeat(800);
        let output = truncate_initial_prompt(&input);
        assert_eq!(output.len(), 512);
    }
}
