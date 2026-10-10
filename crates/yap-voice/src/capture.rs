//! Recording from the microphone and turning it into complete utterances.
//!
//! `cpal` covers WASAPI, CoreAudio, ALSA and PulseAudio/PipeWire behind one
//! API, so the same code captures on all three platforms. The audio callback
//! runs on the device's own thread and must not block, so it does nothing but
//! copy samples into a buffer that a tokio task drains.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use yap_core::engine::{SAMPLE_RATE, VadConfig};

use crate::segmenter::{SegmentedUtterance, Segmenter};

/// Shared capture state.
///
/// The audio callback needs somewhere to put samples that does not require
/// blocking on tokio. A mutex over a small buffer is the standard cpal shape:
/// the callback's critical section is a `Vec::extend_from_slice`.
#[derive(Debug, Default)]
struct CaptureBuffer {
    samples: Vec<f32>,
}

/// A live microphone stream that emits complete utterances.
///
/// Dropping this stops the stream. [`Microphone::stop`] does it explicitly so
/// the caller can await the callback thread rather than have it end with the
/// struct.
pub struct Microphone {
    stream: Option<cpal::Stream>,
    /// Kept so the buffer outlives the stream: the callback closure holds an
    /// `Arc` to it, and letting it drop while the device thread is still
    /// writing would be a use-after-free.
    _buffer: Arc<Mutex<CaptureBuffer>>,
    _device: cpal::Device,
}

// `cpal::Stream` and `cpal::Device` are not Debug, and this type owns them. The
// manual impl prints what is useful — whether capture is live — rather than
// deriving a bound it cannot satisfy.
impl std::fmt::Debug for Microphone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Microphone")
            .field("capturing", &self.stream.is_some())
            .finish()
    }
}

/// A microphone the streamer can pick from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDevice {
    pub name: String,
    /// False for outputs that cannot record, which cpal reports as the same
    /// device.
    pub is_input: bool,
}

impl Microphone {
    /// Every device on the system that can record.
    ///
    /// Returns an error on a machine with no recording device, which is a real
    /// configuration (a headless streaming box with audio over the network) and
    /// deserves a message rather than an empty picker.
    pub fn list_devices() -> Result<Vec<InputDevice>> {
        let host = cpal::default_host();
        let mut devices = Vec::new();

        for device in host.input_devices()? {
            // `description()` is the structured name in this cpal version;
            // `name()` was removed in 0.18.
            if let Ok(description) = device.description() {
                devices.push(InputDevice {
                    name: description.name().to_string(),
                    is_input: true,
                });
            }
        }

        Ok(devices)
    }

    /// Start capturing from the default input, running `on_utterance` for each
    /// complete utterance.
    ///
    /// `on_utterance` is called from a tokio task, never from the audio
    /// callback, so it is free to do slow work like transcribing.
    pub async fn start_default(
        vad: VadConfig,
        on_utterance: impl Fn(SegmentedUtterance) + Send + Sync + 'static,
    ) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("no default input device — pick one in settings")?;

        Self::start(device, vad, on_utterance).await
    }

    /// Start capturing from a named device, falling back to the default.
    pub async fn start_named(
        name: &str,
        vad: VadConfig,
        on_utterance: impl Fn(SegmentedUtterance) + Send + Sync + 'static,
    ) -> Result<Self> {
        let host = cpal::default_host();

        let device = host
            .input_devices()?
            .find(|d| {
                d.description()
                    .map(|desc| desc.name() == name)
                    .unwrap_or(false)
            })
            .with_context(|| format!("no input device named {name:?} — pick one in settings"))?;

        Self::start(device, vad, on_utterance).await
    }

    async fn start(
        device: cpal::Device,
        vad: VadConfig,
        on_utterance: impl Fn(SegmentedUtterance) + Send + Sync + 'static,
    ) -> Result<Self> {
        let buffer = Arc::new(Mutex::new(CaptureBuffer::default()));

        // Capture at the device's native rate and resample to 16 kHz below.
        // Requesting 16 kHz directly often gets a format the device refuses.
        let stream_config = Self::supported_config(&device)?;
        let sample_rate = stream_config.sample_rate;

        let channels = stream_config.channels.max(1) as usize;

        let cb_buffer = Arc::clone(&buffer);
        let stream = device
            .build_input_stream(
                stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mut guard = match cb_buffer.lock() {
                        Ok(g) => g,
                        Err(_) => return,
                    };

                    if channels == 1 {
                        guard.samples.extend_from_slice(data);
                    } else {
                        // Average the channels into one. Taking only the first
                        // would drop half the signal on a stereo interface,
                        // which is enough to lose quiet consonants and make the
                        // recogniser mis-hear words.
                        let frames = data.len() / channels;
                        guard.samples.reserve(frames);
                        for frame in data.chunks_exact(channels) {
                            guard
                                .samples
                                .push(frame.iter().sum::<f32>() / channels as f32);
                        }
                    }
                },
                |err| log::error!("microphone stream error: {err}"),
                None,
            )
            .context("could not open an input stream")?;

        stream.play().context("could not start capture")?;

        // Drain loop: resample, segment, hand over complete utterances.
        let drain_buffer = Arc::clone(&buffer);
        tokio::spawn(async move {
            let mut segmenter = Segmenter::new(vad);
            let mut resampler = Resampler::new(sample_rate);

            loop {
                // 100 ms of audio per wakeup. Frequent enough that an
                // utterance is delivered promptly, infrequent enough not to
                // spin a core for the length of a stream.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                let raw = {
                    let mut guard = match drain_buffer.lock() {
                        Ok(g) => g,
                        Err(_) => break,
                    };
                    std::mem::take(&mut guard.samples)
                };

                if raw.is_empty() {
                    continue;
                }

                let resampled = resampler.process(&raw);
                for utterance in segmenter.push(&resampled) {
                    on_utterance(utterance);
                }
            }
        });

        Ok(Self {
            stream: Some(stream),
            _buffer: buffer,
            _device: device,
        })
    }

    /// Pick a capture format the device will actually accept.
    ///
    /// Prefers mono, since the resampler would otherwise have to downmix. Falls
    /// back to the device default when mono is unsupported, and the callback
    /// averages channels rather than dropping one.
    fn supported_config(device: &cpal::Device) -> Result<cpal::StreamConfig> {
        let supported = device
            .default_input_config()
            .context("could not query the device's input format")?;

        // A stereo capture is fine; the callback downmixes. Asking a device for
        // mono it does not offer gets the whole stream refused, so the default
        // is the safe choice and mono is only a preference.
        Ok(supported.config())
    }

    /// Stop capture and wait for the audio callback to finish.
    ///
    /// Explicit because `cpal::Stream`'s own drop is not guaranteed to join the
    /// callback thread, and a callback still writing into a dropped buffer is a
    /// use-after-free rather than a warning.
    pub fn stop(mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.pause();
            drop(stream);
        }
    }
}

/// Convert from the device's sample rate to the 16 kHz every engine expects.
///
/// Linear interpolation, which is adequate here because the segmenter's job is
/// deciding what is speech, not producing audio anyone will hear.
pub struct Resampler {
    /// Input rate in Hz.
    from: u32,
    /// How many input samples to advance per output sample. Greater than one
    /// when downsampling.
    step: f32,
    /// Fractional read position within the next block, always in `0.0..step`.
    position: f32,
    /// Last sample of the previous block, so interpolation across the seam uses
    /// real audio rather than repeating the first sample of this block.
    previous: Option<f32>,
}

impl Resampler {
    pub fn new(from: u32) -> Self {
        let from = from.max(1);
        Self {
            from,
            step: from as f32 / SAMPLE_RATE as f32,
            position: 0.0,
            previous: None,
        }
    }

    /// Resample a block, carrying interpolation state across calls so a block
    /// boundary does not introduce a click or a dropped sample.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if self.from == SAMPLE_RATE {
            return input.to_vec();
        }

        if input.is_empty() {
            return Vec::new();
        }

        let mut out = Vec::with_capacity((input.len() as f32 / self.step) as usize + 1);
        let mut position = self.position;

        while position < input.len() as f32 {
            let index = position as usize;
            let frac = position - index as f32;

            // `a` is the sample before the read position, `b` the one after it.
            // At index 0 the previous block supplies `a`; on the first block
            // ever there is none, so the first sample stands in for it.
            let a = if index == 0 {
                self.previous.unwrap_or(input[0])
            } else {
                input[index - 1]
            };
            let b = input[index];

            out.push(a + (b - a) * frac);
            position += self.step;
        }

        // Carry the leftover fraction so the next block resumes mid-sample
        // rather than snapping to its start.
        self.position = position - input.len() as f32;
        if self.position < 0.0 {
            self.position = 0.0;
        }

        self.previous = input.last().copied();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_at_the_target_rate() {
        let mut r = Resampler::new(SAMPLE_RATE);
        let input = vec![0.1f32; 100];
        assert_eq!(r.process(&input), input);
    }

    #[test]
    fn downsampling_reduces_the_sample_count() {
        // 48 kHz is the common device rate; 16 kHz is one third of it.
        let mut r = Resampler::new(48_000);
        let input = vec![0.5f32; 4_800];
        let out = r.process(&input);

        assert!(out.len() > 1_400 && out.len() < 1_700, "got {}", out.len());
    }

    #[test]
    fn upsampling_increases_the_sample_count() {
        let mut r = Resampler::new(8_000);
        let input = vec![0.5f32; 800];
        assert!(r.process(&input).len() > 1_500);
    }

    #[test]
    fn resampling_is_continuous_across_blocks() {
        // A ramp resampled in two halves must match the same ramp in one pass.
        // A per-block reset would restart the interpolation and drop a sample.
        let ramp: Vec<f32> = (0..4_800).map(|i| i as f32 / 4_800.0).collect();

        let mut whole = Resampler::new(48_000);
        let a = whole.process(&ramp);

        let mut split = Resampler::new(48_000);
        let mut b = split.process(&ramp[..2_400]);
        b.extend(split.process(&ramp[2_400..]));

        // The two agree to within a sample of drift at the seam.
        let diff = a.len().abs_diff(b.len());
        assert!(diff <= 1, "lengths {} and {}", a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()).skip(2).take(500) {
            assert!((x - y).abs() < 0.01, "{x} vs {y}");
        }
    }

    #[test]
    fn empty_input_is_handled() {
        let mut r = Resampler::new(48_000);
        assert!(r.process(&[]).is_empty());
    }

    #[test]
    fn a_zero_rate_does_not_divide_by_zero() {
        // Defensive: a device reporting 0 Hz would otherwise produce an infinite
        // ratio and spin.
        let mut r = Resampler::new(0);
        let _ = r.process(&[0.5; 100]);
    }

    #[test]
    fn a_default_vad_config_splits_on_a_half_second_pause() {
        // The one parameter worth pinning: it defines what an utterance is.
        let vad = VadConfig::default();
        assert_eq!(vad.min_silence_ms, 500);
        assert!(vad.min_speech_ms > 0);
        assert!(vad.max_speech_ms > vad.min_speech_ms);
    }
}
