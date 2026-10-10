//! The seam between counting logic and speech recognition.
//!
//! Everything above this trait is identical whether words are recognised on
//! this machine or by a paid API. That is the point of drawing the line here:
//! the engine is handed a **complete utterance** and never sees partial audio.
//!
//! Partial audio is the whole correctness problem. A recogniser given a
//! rolling window re-reads overlapping audio and reports the same words twice,
//! which silently inflates the count; counting interim results from a streaming
//! API has the same defect at segment boundaries. Both are avoided by keeping
//! utterance finalisation above the engine, so no engine can be asked to
//! transcribe something that might still change.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One complete utterance: 16 kHz mono float samples, already gated by VAD.
///
/// The sample rate is fixed because whisper.cpp and every hosted STT API expect
/// 16 kHz. Callers resample on capture rather than handing over whatever the
/// device gave them.
#[derive(Debug, Clone)]
pub struct Utterance {
    /// 16 kHz mono samples in `[-1.0, 1.0]`.
    pub samples: Vec<f32>,
    /// Milliseconds from the start of the stream to the first sample.
    pub started_ms: u64,
    /// Milliseconds from the start of the stream to the last sample.
    pub ended_ms: u64,
}

impl Utterance {
    /// Length in milliseconds, derived from sample count.
    pub fn duration_ms(&self) -> u64 {
        self.ended_ms.saturating_sub(self.started_ms)
    }

    /// Length in samples.
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// An empty utterance carries no speech and should never reach an engine.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Build one from samples alone, for tests and fixtures.
    pub fn from_samples(samples: Vec<f32>, started_ms: u64) -> Self {
        let ended_ms = started_ms + (samples.len() as u64 * 1000 / u64::from(SAMPLE_RATE));
        Self {
            samples,
            started_ms,
            ended_ms,
        }
    }
}

/// The sample rate every engine expects.
pub const SAMPLE_RATE: u32 = 16_000;

/// What an engine returns for one utterance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// Recognised text, verbatim. Empty means the engine heard nothing
    /// scoreable, which is normal for a cough or a door slam.
    pub text: String,
    /// Engine confidence in `text`, 0.0..=1.0, when it reports one.
    pub confidence: Option<f32>,
    /// How long the utterance was.
    pub duration: Duration,
    /// Which engine produced this, for the audit log.
    pub engine: String,
}

impl Transcript {
    pub fn new(text: impl Into<String>, engine: &str, duration: Duration) -> Self {
        Self {
            text: text.into(),
            confidence: None,
            duration,
            engine: engine.to_string(),
        }
    }

    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = Some(confidence.clamp(0.0, 1.0));
        self
    }

    /// Whether there is anything to score.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// Failures an engine can report.
///
/// Deliberately coarse. An engine that cannot recognise audio should not be
/// able to fail a yapathon, so these are surfaced in the dock as a warning and
/// the run continues.
#[derive(Debug, Error)]
pub enum EngineError {
    /// The utterance was audio the engine could not use: silent, too short, or
    /// malformed.
    #[error("unusable audio: {0}")]
    UnusableAudio(String),
    /// The model file is missing or unreadable.
    #[error("model unavailable: {0}")]
    ModelUnavailable(String),
    /// The network call failed, or the API refused the key.
    #[error("api error: {0}")]
    Api(String),
    /// Anything else, including panics caught at the boundary.
    #[error("engine failure: {0}")]
    Other(String),
}

/// A speech recognition backend.
///
/// Implementations must be safe to share across the transcription task. They are
/// handed complete utterances only and must not buffer, retry or reorder them:
/// the caller owns ordering, because the goal depends on a total that matches
/// the order words were actually spoken.
#[async_trait::async_trait]
pub trait TranscriptionEngine: Send + Sync {
    /// Recognise one complete utterance.
    async fn transcribe(&self, utterance: Utterance) -> Result<Transcript, EngineError>;

    /// Short identifier for the audit log and the settings UI.
    fn name(&self) -> &'static str;

    /// Whether this engine sends audio off the machine.
    ///
    /// Shown in settings next to the local engines. A streamer choosing a cloud
    /// backend is making a decision about their audio leaving the box, and it
    /// should not be a silent one.
    fn is_remote(&self) -> bool {
        false
    }

    /// Rough cost in US dollars per hour of audio, when known. Local engines
    /// return `None`; hosted ones report their list price so the setting is
    /// comparable rather than a preference.
    fn cost_per_hour_usd(&self) -> Option<f64> {
        None
    }

    /// Release resources. Called on shutdown and when the engine is swapped in
    /// settings, since a loaded model is a large allocation.
    async fn shutdown(&self) {}
}

/// Voice activity detection settings.
///
/// Silence between utterances is what defines an utterance, so these are part
/// of what is counted rather than a detail of capture. Getting them wrong
/// splits one sentence into three and transcribes fragments that whisper then
/// completes from its own prior, inventing words.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VadConfig {
    /// Probability above which a frame is speech.
    pub threshold: f32,
    /// Milliseconds of silence that end an utterance. A pause mid-sentence
    /// should not split it, so this is generous.
    pub min_silence_ms: u32,
    /// Silence shorter than this is ignored as a breath rather than a boundary.
    pub min_silence_ms_ignored: u32,
    /// Speech shorter than this is discarded as a cough or a door.
    pub min_speech_ms: u32,
    /// Utterances longer than this are split, so one long monologue does not
    /// become a single 90-second window that whisper may truncate.
    pub max_speech_ms: u32,
    /// Silence either side of an utterance, to avoid clipping consonants.
    pub speech_pad_ms: u32,
    /// RMS below which audio is treated as silence regardless of VAD. This is
    /// the belt-and-braces against whisper's habit of transcribing nothing.
    pub energy_floor: f32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            // Half a second of pause ends an utterance.
            min_silence_ms: 500,
            min_silence_ms_ignored: 200,
            min_speech_ms: 200,
            max_speech_ms: 15_000,
            speech_pad_ms: 200,
            energy_floor: 0.008,
        }
    }
}

/// RMS energy of a sample buffer.
///
/// Used as the floor under VAD. Whisper will happily transcribe pure silence
/// into an imaginary sentence, and those words would count, so audio that is
/// too quiet to be speech never reaches an engine at all.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Whether a buffer carries enough energy to be speech.
pub fn is_loud_enough(samples: &[f32], floor: f32) -> bool {
    rms(samples) >= floor
}

/// How many samples `ms` is at [`SAMPLE_RATE`].
pub fn samples_for_ms(ms: u32) -> usize {
    (SAMPLE_RATE as u64 * u64::from(ms) / 1000) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_is_derived_from_samples() {
        // One second of audio at 16 kHz.
        let u = Utterance::from_samples(vec![0.0; 16_000], 500);
        assert_eq!(u.duration_ms(), 1000);
        assert_eq!(u.ended_ms, 1500);
        assert!(!u.is_empty());
    }

    #[test]
    fn empty_utterance_is_detected() {
        let u = Utterance::from_samples(vec![], 0);
        assert!(u.is_empty());
        assert_eq!(u.duration_ms(), 0);
    }

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[0.0; 1000]), 0.0);
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn rms_of_a_tone_is_its_amplitude() {
        // A full-scale sine has RMS 1/sqrt(2).
        let samples: Vec<f32> = (0..1000)
            .map(|i| (std::f32::consts::TAU * 100.0 * i as f32 / 1000.0).sin())
            .collect();
        assert!((rms(&samples) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
    }

    #[test]
    fn energy_floor_rejects_quiet_audio() {
        let quiet = vec![0.001f32; 1000];
        let loud = vec![0.5f32; 1000];

        assert!(!is_loud_enough(&quiet, 0.008));
        assert!(is_loud_enough(&loud, 0.008));
    }

    #[test]
    fn samples_for_ms_matches_the_rate() {
        assert_eq!(samples_for_ms(1000), 16_000);
        assert_eq!(samples_for_ms(500), 8_000);
        assert_eq!(samples_for_ms(0), 0);
    }

    #[test]
    fn transcript_confidence_is_clamped() {
        let t = Transcript::new("hello", "test", Duration::from_secs(1)).with_confidence(1.5);
        assert_eq!(t.confidence, Some(1.0));
    }

    #[test]
    fn whitespace_only_transcript_is_empty() {
        assert!(Transcript::new("   ", "test", Duration::ZERO).is_empty());
        assert!(!Transcript::new("hi", "test", Duration::ZERO).is_empty());
    }
}
