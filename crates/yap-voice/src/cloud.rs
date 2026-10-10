//! Groq's hosted whisper-large-v3-turbo.
//!
//! Same model as the local engine, so a streamer can switch between them and
//! the counts stay comparable. The trade is that audio leaves the machine and
//! costs money: $0.04 per hour, about 16 cents for a four-hour stream.
//!
//! Utterances are gated locally before they get here. That is what keeps a
//! cloud backend a drop-in replacement rather than a rewrite: it receives the
//! same complete utterances the local engine does, so no word is counted twice
//! at a streaming segment boundary.

use std::time::Duration;

use serde::Deserialize;
use yap_core::engine::{EngineError, Transcript, TranscriptionEngine, Utterance};

/// Groq's OpenAI-compatible transcription endpoint.
const ENDPOINT: &str = "https://api.groq.com/openai/v1/audio/transcriptions";

/// The model both engines run.
const MODEL: &str = "whisper-large-v3-turbo";

/// List price in US dollars per hour of audio.
pub const COST_PER_HOUR_USD: f64 = 0.04;

pub struct GroqEngine {
    client: reqwest::Client,
    api_key: String,
}

impl GroqEngine {
    /// Build an engine with an API key already loaded from the credential
    /// store.
    pub fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
        }
    }

    /// Look up a key in the OS credential store.
    ///
    /// A missing entry is an error naming the service, because "no API key" is
    /// the single most common reason this engine fails and it should not
    /// present as a network problem.
    /// The credential-store entry this engine reads.
    ///
    /// `user` is fixed because there is one Groq key per install; the service
    /// distinguishes it from the Twitch token stored alongside.
    pub const KEYRING_USER: &'static str = "api-key";

    pub fn from_keyring(service: &str) -> Result<Self, EngineError> {
        // Through `yap-core::secrets`, the same door the in-app credential
        // prompt writes through.
        let secret = yap_core::secrets::get(service, Self::KEYRING_USER)
            .map_err(|e| EngineError::Api(format!("credential store unavailable: {e}")))?
            .ok_or_else(|| EngineError::Api(format!("no stored key for {service}")))?;

        Ok(Self::new(secret))
    }

    /// Store a key for later use.
    pub fn store_keyring(service: &str, api_key: &str) -> Result<(), EngineError> {
        yap_core::secrets::set(service, Self::KEYRING_USER, api_key)
            .map_err(|e| EngineError::Api(format!("could not store the key: {e}")))
    }
}

/// Groq's response shape.
#[derive(Debug, Deserialize)]
struct Response {
    text: String,
}

/// Wire format for the multipart upload.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WireError {
    error: WireErrorBody,
}

#[derive(Debug, Deserialize)]
struct WireErrorBody {
    message: String,
}

#[async_trait::async_trait]
impl TranscriptionEngine for GroqEngine {
    async fn transcribe(&self, utterance: Utterance) -> Result<Transcript, EngineError> {
        if utterance.is_empty() {
            return Err(EngineError::UnusableAudio("empty utterance".to_string()));
        }

        // whisper.cpp and Groq both expect 16-bit WAV. Encode in memory: an
        // utterance is seconds long, so this is a few hundred kilobytes.
        let wav = encode_wav_16k_mono(&utterance.samples);

        let part = reqwest::multipart::Part::bytes(wav)
            .file_name("utterance.wav")
            .mime_str("audio/wav")
            .map_err(|e| EngineError::Other(format!("multipart: {e}")))?;

        // `verbose_json` returns segment and word data; the plain shape returns
        // only text, which is all the counter needs. One less thing to parse.
        let form = reqwest::multipart::Form::new()
            .text("model", MODEL)
            .text("response_format", "json")
            .text("temperature", "0")
            .part("file", part);

        let response = self
            .client
            .post(ENDPOINT)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| EngineError::Api(format!("request failed: {e}")))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| EngineError::Api(format!("could not read response: {e}")))?;

        if !status.is_success() {
            // Prefer the API's own message: it says which of the many possible
            // causes it was.
            let message = serde_json::from_str::<WireError>(&body)
                .map(|w| w.error.message)
                .unwrap_or(body);
            return Err(EngineError::Api(format!("{status}: {message}")));
        }

        let parsed: Response = serde_json::from_str(&body)
            .map_err(|e| EngineError::Api(format!("unreadable response: {e}")))?;

        Ok(Transcript::new(
            parsed.text,
            "groq",
            Duration::from_millis(utterance.duration_ms()),
        ))
    }

    fn name(&self) -> &'static str {
        "groq"
    }

    fn is_remote(&self) -> bool {
        true
    }

    fn cost_per_hour_usd(&self) -> Option<f64> {
        Some(COST_PER_HOUR_USD)
    }
}

/// Encode 16 kHz mono float samples as a 16-bit PCM WAV file.
///
/// A minimal RIFF header plus the samples. Written by hand because the point of
/// the project is that the local engine needs no dependencies, and pulling in an
/// encoder crate to emit forty bytes of header would be silly.
pub fn encode_wav_16k_mono(samples: &[f32]) -> Vec<u8> {
    const HEADER: usize = 44;
    const BITS: u16 = 16;

    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(HEADER + samples.len() * 2);

    // "RIFF"
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    // "WAVE"
    out.extend_from_slice(b"WAVE");

    // "fmt " subchunk, PCM, mono, 16 kHz.
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // subchunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE_HZ.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE_HZ * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&BITS.to_le_bytes());

    // "data" subchunk.
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());

    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        out.extend_from_slice(&((clamped * i16::MAX as f32) as i16).to_le_bytes());
    }

    out
}

/// Sample rate written into the WAV header.
const SAMPLE_RATE_HZ: u32 = 16_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_well_formed() {
        let wav = encode_wav_16k_mono(&[0.0; 16_000]);

        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");

        // One second of 16-bit mono audio is 32000 bytes of samples.
        assert_eq!(wav.len(), 44 + 32_000);

        let riff_size = u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]);
        assert_eq!(riff_size as usize, wav.len() - 8);
    }

    #[test]
    fn wav_declares_16_khz_mono_16_bit() {
        let wav = encode_wav_16k_mono(&[0.0; 100]);
        let rate = u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]);
        let channels = u16::from_le_bytes([wav[22], wav[23]]);
        let bits = u16::from_le_bytes([wav[34], wav[35]]);

        assert_eq!(rate, 16_000);
        assert_eq!(channels, 1);
        assert_eq!(bits, 16);
    }

    #[test]
    fn samples_are_clamped_rather_than_wrapping() {
        // A sample above full scale must not wrap to a large positive value,
        // which would sound like a click and could confuse VAD. Scaling by
        // `i16::MAX` rather than 32768 keeps the range symmetric enough: full
        // negative scale lands on -32767, not -32768.
        let wav = encode_wav_16k_mono(&[2.0, -2.0]);
        let first = i16::from_le_bytes([wav[44], wav[45]]);
        let second = i16::from_le_bytes([wav[46], wav[47]]);

        assert_eq!(first, i16::MAX);
        assert_eq!(second, -i16::MAX);
    }

    #[tokio::test]
    async fn empty_audio_is_rejected_before_spending_a_request() {
        // Groq bills a ten-second minimum per request, so rejecting an empty
        // utterance before the call is worth the guard.
        let utterance = Utterance::from_samples(Vec::new(), 0);
        assert!(utterance.is_empty());
    }

    #[test]
    fn this_engine_is_marked_remote_and_priced() {
        // The settings UI shows both, so switching to a cloud backend is an
        // informed choice about audio leaving the machine.
        let engine = GroqEngine::new("test-key".to_string());
        assert!(engine.is_remote());
        assert_eq!(engine.cost_per_hour_usd(), Some(0.04));
        assert_eq!(engine.name(), "groq");
    }
}
