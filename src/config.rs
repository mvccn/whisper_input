//! Command-line configuration for whisper_input.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;

use crate::model::{self, ModelSize};

/// CLI arguments for the whisper_input binary.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "whisper_input",
    about = "Local Whisper voice input for terminal apps on macOS"
)]
pub(crate) struct Cli {
    /// Whisper model size preset to use.
    #[arg(
        long,
        env = "WHISPER_MODEL_SIZE",
        value_enum,
        default_value_t = ModelSize::Base
    )]
    pub(crate) model_size: ModelSize,

    /// Directory where Whisper model files are cached.
    #[arg(
        long,
        env = "WHISPER_MODEL_DIR",
        default_value_os_t = model::default_model_dir()
    )]
    pub(crate) model_dir: PathBuf,

    /// CPU threads used by Whisper.
    #[arg(long, env = "WHISPER_THREADS")]
    pub(crate) threads: Option<usize>,

    /// Maximum length in seconds for one recording window.
    #[arg(long, default_value_t = 45)]
    pub(crate) max_record_seconds: u64,

    /// Maximum press duration in milliseconds for a left-command tap.
    #[arg(long, default_value_t = 450)]
    pub(crate) hotkey_max_tap_ms: u64,

    /// Disable GPU acceleration and force CPU-only inference.
    #[arg(long, env = "WHISPER_NO_GPU")]
    pub(crate) no_gpu: bool,

    /// Disable Flash Attention in Whisper context initialization.
    #[arg(long, env = "WHISPER_NO_FLASH_ATTN")]
    pub(crate) no_flash_attn: bool,

    /// Do not auto-paste after transcription; copy to clipboard only.
    #[arg(long)]
    pub(crate) no_auto_paste: bool,

    /// Run one non-interactive capture/transcribe pass and print transcript to stdout.
    #[arg(long)]
    pub(crate) tool_transcribe_once: bool,

    /// Capture duration in seconds for `--tool-transcribe-once`.
    #[arg(long, default_value_t = 6)]
    pub(crate) tool_record_seconds: u64,

    /// Inline initial prompt text for `--tool-transcribe-once`.
    #[arg(long)]
    pub(crate) tool_initial_prompt: Option<String>,

    /// File containing initial prompt text for `--tool-transcribe-once`.
    #[arg(long)]
    pub(crate) tool_initial_prompt_file: Option<PathBuf>,
}

/// Runtime config resolved from CLI and environment.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) model_size: ModelSize,
    pub(crate) model_dir: PathBuf,
    pub(crate) threads: i32,
    pub(crate) max_record_seconds: u64,
    pub(crate) hotkey_max_tap_ms: u64,
    pub(crate) use_gpu: bool,
    pub(crate) flash_attn: bool,
    pub(crate) auto_paste: bool,
}

impl Config {
    /// Validates CLI values and produces a normalized runtime config.
    ///
    /// # Errors
    /// Returns an error when required values are missing, invalid, or
    /// impossible to use at runtime.
    pub(crate) fn from_cli(cli: Cli) -> Result<Self> {
        if cli.max_record_seconds == 0 {
            bail!("--max-record-seconds must be at least 1");
        }
        if cli.hotkey_max_tap_ms == 0 {
            bail!("--hotkey-max-tap-ms must be at least 1");
        }
        if cli.model_dir.as_os_str().is_empty() {
            bail!("--model-dir must not be empty");
        }
        if cli.tool_transcribe_once && cli.tool_record_seconds == 0 {
            bail!("--tool-record-seconds must be at least 1");
        }

        let threads = normalize_threads(cli.threads)?;

        Ok(Self {
            model_size: cli.model_size,
            model_dir: cli.model_dir,
            threads,
            max_record_seconds: cli.max_record_seconds,
            hotkey_max_tap_ms: cli.hotkey_max_tap_ms,
            use_gpu: !cli.no_gpu,
            flash_attn: !cli.no_flash_attn,
            auto_paste: !cli.no_auto_paste,
        })
    }
}

impl Cli {
    /// Resolves initial prompt text for one-shot tool mode from inline and file sources.
    ///
    /// # Errors
    /// Returns an error when `tool_initial_prompt_file` cannot be read.
    pub(crate) fn resolve_tool_initial_prompt(&self) -> Result<Option<String>> {
        let mut parts = Vec::new();

        if let Some(inline) = self
            .tool_initial_prompt
            .as_deref()
            .and_then(non_empty_trimmed)
        {
            parts.push(inline.to_string());
        }

        if let Some(path) = self.tool_initial_prompt_file.as_deref() {
            let file_prompt = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read prompt file {}", path.display()))?;
            if let Some(text) = non_empty_trimmed(file_prompt.as_str()) {
                parts.push(text.to_string());
            }
        }

        if parts.is_empty() {
            return Ok(None);
        }

        Ok(Some(parts.join("\n")))
    }
}

/// Returns a trimmed string slice when the input is non-empty.
fn non_empty_trimmed(input: &str) -> Option<&str> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed)
}

/// Resolves thread count with a robust default for local inference.
///
/// # Errors
/// Returns an error when the thread count is zero or larger than `i32::MAX`.
fn normalize_threads(input: Option<usize>) -> Result<i32> {
    let default_threads = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4);
    let chosen = input.unwrap_or(default_threads);

    if chosen == 0 {
        bail!("--threads must be at least 1");
    }

    i32::try_from(chosen).context("thread count is too large")
}

#[cfg(test)]
mod tests {
    use super::{Cli, Config, normalize_threads};
    use crate::model::ModelSize;
    use clap::Parser;
    use std::fs;

    fn valid_cli() -> Cli {
        Cli::parse_from([
            "whisper_input",
            "--model-size",
            "small",
            "--model-dir",
            "/tmp/whisper_models",
            "--threads",
            "2",
        ])
    }

    #[test]
    fn normalize_threads_rejects_zero() {
        let err = normalize_threads(Some(0)).expect_err("zero threads should fail");
        assert!(err.to_string().contains("at least 1"));
    }

    #[test]
    fn cli_default_model_size_is_base() {
        let cli = Cli::parse_from(["whisper_input"]);
        assert_eq!(cli.model_size, ModelSize::Base);
    }

    #[test]
    fn from_cli_maps_fields() {
        let cli = valid_cli();
        let config = Config::from_cli(cli).expect("cli should parse");

        assert_eq!(config.model_size, ModelSize::Small);
        assert_eq!(
            config.model_dir,
            std::path::PathBuf::from("/tmp/whisper_models")
        );
        assert_eq!(config.threads, 2);
        assert!(config.use_gpu);
        assert!(config.flash_attn);
        assert!(config.auto_paste);
    }

    #[test]
    fn from_cli_allows_disabling_gpu_features() {
        let cli = Cli::parse_from([
            "whisper_input",
            "--model-size",
            "small",
            "--model-dir",
            "/tmp/whisper_models",
            "--no-gpu",
            "--no-flash-attn",
        ]);
        let config = Config::from_cli(cli).expect("cli should parse");

        assert!(!config.use_gpu);
        assert!(!config.flash_attn);
    }

    #[test]
    fn from_cli_rejects_zero_tool_record_seconds_when_tool_mode() {
        let cli = Cli::parse_from([
            "whisper_input",
            "--model-size",
            "small",
            "--model-dir",
            "/tmp/whisper_models",
            "--tool-transcribe-once",
            "--tool-record-seconds",
            "0",
        ]);

        let err = Config::from_cli(cli).expect_err("tool mode with zero seconds should fail");
        assert!(err.to_string().contains("--tool-record-seconds"));
    }

    #[test]
    fn resolve_tool_initial_prompt_combines_inline_and_file() {
        let prompt_file = tempfile::NamedTempFile::new().expect("temp file should be created");
        fs::write(prompt_file.path(), "beta gamma").expect("temp prompt file should be writable");

        let cli = Cli::parse_from([
            "whisper_input",
            "--tool-initial-prompt",
            "alpha",
            "--tool-initial-prompt-file",
            prompt_file.path().to_str().expect("utf8 temp path"),
        ]);

        let prompt = cli
            .resolve_tool_initial_prompt()
            .expect("prompt resolution should succeed");
        assert_eq!(prompt, Some(String::from("alpha\nbeta gamma")));
    }

    #[test]
    fn resolve_tool_initial_prompt_skips_empty_sources() {
        let cli = Cli::parse_from(["whisper_input", "--tool-initial-prompt", "   "]);
        let prompt = cli
            .resolve_tool_initial_prompt()
            .expect("prompt resolution should succeed");
        assert_eq!(prompt, None);
    }
}
