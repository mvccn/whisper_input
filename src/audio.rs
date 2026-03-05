//! Audio capture and conversion utilities.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig, SupportedStreamConfig,
};
use tracing::{error, info};

/// Whisper input sample rate.
pub(crate) const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Captured mono samples and source sample rate.
#[derive(Debug, Clone)]
pub(crate) struct CapturedAudio {
    pub(crate) samples: Vec<f32>,
    pub(crate) sample_rate: u32,
}

/// Mic recorder bound to the default input device.
pub(crate) struct Recorder {
    device: cpal::Device,
    supported_config: SupportedStreamConfig,
    max_record_seconds: u64,
}

/// Active recording guard. Dropping this guard stops the stream.
pub(crate) struct ActiveRecording {
    stream: Stream,
    shared: Arc<SharedCapture>,
    sample_rate: u32,
}

#[derive(Debug)]
struct SharedCapture {
    samples: Mutex<Vec<f32>>,
    stop: AtomicBool,
    max_samples: usize,
}

impl Recorder {
    /// Creates a recorder for the system's default input device.
    ///
    /// # Errors
    /// Returns an error if no input device or input config is available.
    pub(crate) fn new(max_record_seconds: u64) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("no default input device found"))?;
        let supported_config = device
            .default_input_config()
            .map_err(|err| anyhow!("failed to get input config: {err}"))?;

        info!(
            sample_rate = supported_config.sample_rate(),
            channels = supported_config.channels(),
            sample_format = ?supported_config.sample_format(),
            "initialized audio recorder"
        );

        Ok(Self {
            device,
            supported_config,
            max_record_seconds,
        })
    }

    /// Starts capturing microphone audio until the caller stops the session.
    ///
    /// # Errors
    /// Returns an error when stream construction or startup fails.
    pub(crate) fn start(&self) -> Result<ActiveRecording> {
        let stream_config: StreamConfig = self.supported_config.clone().into();
        let sample_rate = stream_config.sample_rate;
        let channels = usize::from(stream_config.channels);
        let max_samples = sample_rate as usize * self.max_record_seconds as usize;
        let shared = Arc::new(SharedCapture {
            samples: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
            max_samples,
        });

        let stream = match self.supported_config.sample_format() {
            SampleFormat::F32 => {
                build_input_stream::<f32>(&self.device, &stream_config, shared.clone(), channels)?
            }
            SampleFormat::I16 => {
                build_input_stream::<i16>(&self.device, &stream_config, shared.clone(), channels)?
            }
            SampleFormat::U16 => {
                build_input_stream::<u16>(&self.device, &stream_config, shared.clone(), channels)?
            }
            other => {
                bail!("unsupported input sample format: {other:?}");
            }
        };

        stream
            .play()
            .map_err(|err| anyhow!("failed to start input stream: {err}"))?;

        Ok(ActiveRecording {
            stream,
            shared,
            sample_rate,
        })
    }
}

impl ActiveRecording {
    /// Stops recording and returns captured mono samples.
    pub(crate) fn stop(self) -> CapturedAudio {
        self.shared.stop.store(true, Ordering::Relaxed);
        drop(self.stream);

        let samples = self
            .shared
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        CapturedAudio {
            samples,
            sample_rate: self.sample_rate,
        }
    }
}

/// Converts captured mono audio to Whisper-ready 16kHz mono.
pub(crate) fn to_whisper_samples(audio: &CapturedAudio) -> Vec<f32> {
    resample_linear(&audio.samples, audio.sample_rate, TARGET_SAMPLE_RATE)
}

/// Checks that audio is long enough and non-silent before inference.
pub(crate) fn has_minimum_signal(samples: &[f32]) -> bool {
    if samples.len() < 800 {
        return false;
    }

    let sum_sq: f32 = samples.iter().map(|sample| sample * sample).sum();
    let rms = (sum_sq / samples.len() as f32).sqrt();
    rms >= 0.002
}

/// Resamples mono audio with linear interpolation.
pub(crate) fn resample_linear(input: &[f32], from_hz: u32, to_hz: u32) -> Vec<f32> {
    if input.is_empty() || from_hz == to_hz {
        return input.to_vec();
    }

    let ratio = to_hz as f64 / from_hz as f64;
    let out_len = (input.len() as f64 * ratio).max(1.0) as usize;
    let mut output = Vec::with_capacity(out_len);

    for out_index in 0..out_len {
        let source_pos = out_index as f64 / ratio;
        let left = source_pos.floor() as usize;
        let right = (left + 1).min(input.len().saturating_sub(1));
        let frac = (source_pos - left as f64) as f32;

        let value = input[left] * (1.0 - frac) + input[right] * frac;
        output.push(value);
    }

    output
}

/// Builds an input stream for a specific sample type and forwards mono data into shared storage.
fn build_input_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    shared: Arc<SharedCapture>,
    channels: usize,
) -> Result<Stream>
where
    T: Sample + SizedSample + Copy + Send + 'static,
    f32: FromSample<T>,
{
    let err_fn = move |err| {
        error!(error = %err, "audio stream error");
    };

    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| write_input_data::<T>(data, &shared, channels),
        err_fn,
        None,
    )?;

    Ok(stream)
}

/// Downmixes frames to mono and appends them to the shared sample buffer.
fn write_input_data<T>(input: &[T], shared: &SharedCapture, channels: usize)
where
    T: Sample + Copy,
    f32: FromSample<T>,
{
    if shared.stop.load(Ordering::Relaxed) {
        return;
    }

    let mut guard = shared
        .samples
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    for frame in input.chunks(channels.max(1)) {
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
        if guard.len() >= shared.max_samples {
            shared.stop.store(true, Ordering::Relaxed);
            break;
        }

        let sum: f32 = frame
            .iter()
            .copied()
            .map(f32::from_sample)
            .fold(0.0, |acc, sample| acc + sample);
        guard.push(sum / frame.len() as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::{has_minimum_signal, resample_linear};

    #[test]
    fn resample_same_rate_is_identity() {
        let input = vec![0.1, 0.2, 0.3, 0.4];
        assert_eq!(resample_linear(&input, 16_000, 16_000), input);
    }

    #[test]
    fn resample_downsample_reduces_length() {
        let input = vec![1.0; 480];
        let out = resample_linear(&input, 48_000, 16_000);
        assert!(out.len() < input.len());
        assert!(!out.is_empty());
    }

    #[test]
    fn low_energy_audio_is_rejected() {
        let silence = vec![0.0_f32; 5000];
        assert!(!has_minimum_signal(&silence));
    }

    #[test]
    fn speech_like_audio_is_accepted() {
        let voice = vec![0.02_f32; 5000];
        assert!(has_minimum_signal(&voice));
    }
}
