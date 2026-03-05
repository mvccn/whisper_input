//! Whisper model selection and download/cache management.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::ValueEnum;
use tracing::info;

/// Supported Whisper model-size presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ModelSize {
    Tiny,
    Base,
    Small,
    Medium,
    Large,
}

impl ModelSize {
    /// Returns the expected model filename for a size preset.
    fn filename(self) -> &'static str {
        match self {
            Self::Tiny => "ggml-tiny.en.bin",
            Self::Base => "ggml-base.en.bin",
            Self::Small => "ggml-small.en.bin",
            Self::Medium => "ggml-medium.en.bin",
            // Large does not have an `.en` variant in whisper.cpp assets.
            Self::Large => "ggml-large-v3.bin",
        }
    }

    /// Returns the download URL for a size preset.
    fn download_url(self) -> String {
        format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
            self.filename()
        )
    }
}

/// Resolves the default model cache directory.
pub(crate) fn default_model_dir() -> PathBuf {
    if let Some(cache_dir) = dirs::cache_dir() {
        return cache_dir.join("whisper_input").join("models");
    }

    if let Some(home_dir) = dirs::home_dir() {
        return home_dir.join(".cache").join("whisper_input").join("models");
    }

    PathBuf::from(".whisper_input/models")
}

/// Ensures a local model file exists by downloading it when missing.
///
/// # Errors
/// Returns an error when directories cannot be created or network/file IO fails.
pub(crate) fn ensure_model(size: ModelSize, model_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(model_dir).with_context(|| {
        format!(
            "failed to create model directory at {}",
            model_dir.display()
        )
    })?;

    let model_path = model_dir.join(size.filename());
    if model_path.exists() {
        return Ok(model_path);
    }

    let temp_path = temp_download_path(&model_path);
    if temp_path.exists() {
        fs::remove_file(&temp_path).with_context(|| {
            format!(
                "failed to remove previous temp model file {}",
                temp_path.display()
            )
        })?;
    }

    let url = size.download_url();
    info!(
        size = ?size,
        destination = %model_path.display(),
        "model not found locally; downloading"
    );

    if let Err(err) = download_to_file(&url, &temp_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }

    fs::rename(&temp_path, &model_path).with_context(|| {
        format!(
            "failed to move downloaded model into place: {} -> {}",
            temp_path.display(),
            model_path.display()
        )
    })?;

    info!(
        size = ?size,
        destination = %model_path.display(),
        "model download complete"
    );

    Ok(model_path)
}

/// Builds a stable temp file path next to the final model file.
fn temp_download_path(model_path: &Path) -> PathBuf {
    let file_name = model_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("model.bin"));

    model_path.with_file_name(format!("{file_name}.download"))
}

/// Downloads a model file to disk using a blocking HTTP client.
///
/// # Errors
/// Returns an error when HTTP status is non-success or file IO fails.
fn download_to_file(url: &str, destination: &Path) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60 * 60))
        .build()
        .context("failed to build HTTP client")?;

    let mut response = client
        .get(url)
        .send()
        .with_context(|| format!("failed to download model from {url}"))?
        .error_for_status()
        .with_context(|| format!("model server returned error for {url}"))?;

    let file = File::create(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    let mut writer = BufWriter::new(file);

    let bytes = response
        .copy_to(&mut writer)
        .map_err(|err| anyhow!("failed while writing model file: {err}"))?;
    writer
        .flush()
        .with_context(|| format!("failed to flush {}", destination.display()))?;

    info!(bytes, destination = %destination.display(), "model bytes downloaded");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ModelSize, ensure_model};

    #[test]
    fn filenames_match_expected_assets() {
        assert_eq!(ModelSize::Tiny.filename(), "ggml-tiny.en.bin");
        assert_eq!(ModelSize::Base.filename(), "ggml-base.en.bin");
        assert_eq!(ModelSize::Small.filename(), "ggml-small.en.bin");
        assert_eq!(ModelSize::Medium.filename(), "ggml-medium.en.bin");
        assert_eq!(ModelSize::Large.filename(), "ggml-large-v3.bin");
    }

    #[test]
    fn ensure_model_uses_existing_file_without_network() {
        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let model_path = temp_dir.path().join("ggml-small.en.bin");
        std::fs::write(&model_path, b"existing").expect("existing model should be writable");

        let resolved = ensure_model(ModelSize::Small, temp_dir.path())
            .expect("existing model should be reused");

        assert_eq!(resolved, model_path);
        assert_eq!(
            std::fs::read(&resolved).expect("existing model should still be readable"),
            b"existing"
        );
    }
}
