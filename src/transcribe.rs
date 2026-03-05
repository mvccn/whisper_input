//! Whisper model loading and transcription helpers.

use std::path::Path;
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use tracing::info;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Wrapper around a loaded Whisper context.
pub(crate) struct WhisperEngine {
    context: WhisperContext,
    threads: i32,
    use_gpu: bool,
    flash_attn: bool,
}

impl WhisperEngine {
    /// Loads a Whisper model from disk.
    ///
    /// # Errors
    /// Returns an error when the model cannot be opened or parsed.
    pub(crate) fn new(
        model_path: &Path,
        threads: i32,
        use_gpu: bool,
        flash_attn: bool,
    ) -> Result<Self> {
        let mut context_params = WhisperContextParameters::default();
        context_params.use_gpu(use_gpu).flash_attn(flash_attn);

        let context =
            WhisperContext::new_with_params(&model_path.to_string_lossy(), context_params)
                .map_err(|err| anyhow!("failed to load whisper model: {err}"))?;

        info!(
            model_path = %model_path.display(),
            threads,
            use_gpu,
            flash_attn,
            "initialized whisper engine"
        );

        Ok(Self {
            context,
            threads,
            use_gpu,
            flash_attn,
        })
    }

    /// Runs full transcription over 16kHz mono float samples.
    ///
    /// # Errors
    /// Returns an error when decoding fails or the model state cannot be created.
    pub(crate) fn transcribe(&self, audio: &[f32]) -> Result<String> {
        self.transcribe_with_prompt(audio, None)
    }

    /// Runs full transcription over 16kHz mono float samples with optional prompt bias.
    ///
    /// # Errors
    /// Returns an error when decoding fails or the model state cannot be created.
    pub(crate) fn transcribe_with_prompt(
        &self,
        audio: &[f32],
        initial_prompt: Option<&str>,
    ) -> Result<String> {
        if audio.is_empty() {
            bail!("cannot transcribe empty audio buffer");
        }
        let started_at = Instant::now();

        let mut state = self
            .context
            .create_state()
            .map_err(|err| anyhow!("failed to create whisper state: {err}"))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 0 });
        params.set_n_threads(self.threads);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_timestamps(false);
        params.set_translate(false);
        params.set_language(Some("en"));
        params.set_no_timestamps(true);
        params.set_single_segment(true);
        if let Some(prompt) = initial_prompt.and_then(non_empty_trimmed) {
            let sanitized_prompt = sanitize_initial_prompt(prompt);
            params.set_initial_prompt(&sanitized_prompt);
        }

        state
            .full(params, audio)
            .map_err(|err| anyhow!("whisper decode failed: {err}"))?;

        let mut raw_text = String::new();
        for segment in state.as_iter() {
            raw_text.push_str(&segment.to_string());
        }

        let elapsed = started_at.elapsed();
        let audio_seconds = audio.len() as f64 / 16_000.0;
        let rtf = if audio_seconds > 0.0 {
            elapsed.as_secs_f64() / audio_seconds
        } else {
            0.0
        };
        info!(
            audio_seconds,
            decode_ms = elapsed.as_millis(),
            real_time_factor = rtf,
            use_gpu = self.use_gpu,
            flash_attn = self.flash_attn,
            "transcription complete"
        );

        Ok(normalize_transcript(&raw_text))
    }
}

/// Normalizes whitespace for paste-friendly output.
fn normalize_transcript(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

/// Returns a trimmed string slice when the input is non-empty.
fn non_empty_trimmed(input: &str) -> Option<&str> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed)
}

/// Replaces null bytes because whisper-rs prompt APIs require C-compatible strings.
fn sanitize_initial_prompt(input: &str) -> String {
    if !input.contains('\0') {
        return input.to_string();
    }
    input.replace('\0', " ")
}

#[cfg(test)]
mod tests {
    use super::{normalize_transcript, sanitize_initial_prompt};

    #[test]
    fn normalization_compacts_whitespace() {
        let input = "  hello\n   world   ";
        assert_eq!(normalize_transcript(input), "hello world");
    }

    #[test]
    fn prompt_sanitization_removes_null_bytes() {
        let input = "hello\0world";
        assert_eq!(sanitize_initial_prompt(input), "hello world");
    }
}
