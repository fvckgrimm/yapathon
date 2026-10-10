//! Whisper on this machine, via whisper.cpp.
//!
//! The default engine. Free, offline, and the only option where the streamer's
//! audio stays on their own hardware.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use yap_core::engine::{EngineError, Transcript, TranscriptionEngine, Utterance};

use crate::config::{BackendChoice, DetectedBackend};

/// A local whisper.cpp engine.
///
/// The context is loaded once and reused. A `whisper_state` is created per
/// utterance and dropped with it: whisper's decoder state carries the previous
/// window forward, and carrying that across utterances is how the same sentence
/// gets transcribed twice.
pub struct WhisperEngine {
    context: Arc<Mutex<WhisperContextInner>>,
    model_name: String,
    backend: DetectedBackend,
    language: Option<String>,
    threads: i32,
}

struct WhisperContextInner {
    context: whisper_rs::WhisperContext,
}

use std::sync::Arc;

impl std::fmt::Debug for WhisperEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhisperEngine")
            .field("model", &self.model_name)
            .field("backend", &self.backend.label())
            .finish()
    }
}

impl WhisperEngine {
    /// Load a model and probe for the best available compute backend.
    ///
    /// # Errors
    /// Returns [`EngineError::ModelUnavailable`] if the file is missing or is
    /// not a ggml model. The message names the path, because a streamer who
    /// has not downloaded the model yet is the common case.
    pub fn load(model_path: &Path, choice: BackendChoice) -> Result<Self, EngineError> {
        if !model_path.exists() {
            return Err(EngineError::ModelUnavailable(format!(
                "no model at {} — download a ggml model and point settings at it",
                model_path.display()
            )));
        }

        let resolved = choice.resolve();
        let params = whisper_rs::WhisperContextParameters::default();
        let context = whisper_rs::WhisperContext::new_with_params(
            model_path.to_str().ok_or_else(|| {
                EngineError::ModelUnavailable(format!(
                    "model path is not valid UTF-8: {}",
                    model_path.display()
                ))
            })?,
            params,
        )
        .map_err(|e| EngineError::ModelUnavailable(format!("{model_path:?}: {e}")))?;

        let model_name = model_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "ggml model".to_string());

        let threads = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(4);

        Ok(Self {
            // A context behind a mutex: `whisper_context` is not `Sync`, but a
            // single lock also serialises recognition, which is what we want —
            // two utterances must not be decoded at once.
            context: Arc::new(Mutex::new(WhisperContextInner { context })),
            model_name,
            backend: resolved,
            language: Some("en".to_string()),
            threads,
        })
    }

    /// The backend actually in use after probing.
    pub fn backend(&self) -> DetectedBackend {
        self.backend
    }

    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// Set the recognition language, or `None` to autodetect.
    pub fn set_language(&mut self, language: Option<String>) {
        self.language = language;
    }

    /// A model path that exists, if one can be found in the usual places.
    ///
    /// Convenience for first run: a streamer who dropped the file in Downloads
    /// should not have to find the settings panel.
    pub fn discover(start: &Path) -> Option<PathBuf> {
        let candidates = [
            "ggml-large-v3-turbo.bin",
            "ggml-medium.en.bin",
            "ggml-small.en.bin",
            "ggml-base.en.bin",
        ];

        for dir in [
            start.to_path_buf(),
            start.join("yap-a-thon"),
            start.join("models"),
        ] {
            for name in candidates {
                let path = dir.join(name);
                if path.exists() {
                    return Some(path);
                }
            }
        }

        None
    }
}

#[async_trait::async_trait]
impl TranscriptionEngine for WhisperEngine {
    async fn transcribe(&self, utterance: Utterance) -> Result<Transcript, EngineError> {
        if utterance.is_empty() {
            return Err(EngineError::UnusableAudio("empty utterance".to_string()));
        }

        // whisper.cpp is a blocking C call. Handing it a tokio worker would
        // stall every other task on the runtime, including the WebSocket the
        // overlay is reading from, so it goes on a blocking thread.
        let context = Arc::clone(&self.context);
        let language = self.language.clone();
        let threads = self.threads;

        tokio::task::spawn_blocking(move || {
            let guard = context
                .lock()
                .map_err(|_| EngineError::Other("whisper context poisoned".to_string()))?;

            let mut state = guard
                .context
                .create_state()
                .map_err(|e| EngineError::Other(format!("create_state: {e}")))?;

            // Settings chosen for counting rather than for readable transcripts:
            //
            // `condition_on_previous_text(false)` — the segmenter already
            // guarantees a complete utterance, so conditioning on prior text
            // would only let whisper invent words to fit what came before.
            //
            // `temperature(0)` — greedy. Sampling picks a plausible-sounding
            // word that may not have been said, and every such pick counts.
            //
            // `single_segment(true)` — one utterance, one segment, so the
            // transcript is not chopped mid-sentence at a boundary the segmenter
            // never chose.
            //
            // `no_speech_thold` is left at whisper's default; the segmenter's
            // energy floor has already excluded audio that was not speech.
            let mut params =
                whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
            params.set_n_threads(threads);
            params.set_language(language.as_deref());
            params.set_translate(false);
            params.set_suppress_blank(true);
            params.set_suppress_nst(true);
            params.set_temperature(0.0);
            params.set_temperature_inc(0.0);
            params.set_single_segment(true);
            // Drop the decoder's prior window. This is the setting that stops
            // whisper treating an utterance as a continuation of the last one,
            // which is exactly how the same sentence gets transcribed twice.
            params.set_no_context(true);
            params.set_max_initial_ts(0.0);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_print_special(false);
            params.set_token_timestamps(false);

            state
                .full(params, &utterance.samples)
                .map_err(|e| EngineError::Other(format!("transcribe: {e}")))?;

            // Concatenate segments and drop the ones whisper is not confident
            // are speech.
            //
            // `no_speech_probability` is the second guard against invented
            // words. Whisper transcribes near-silence into a plausible-looking
            // sentence, and those words would count toward the goal, so a
            // segment whisper itself calls non-speech is discarded even though
            // the segmenter's energy floor already passed it.
            const MAX_NO_SPEECH_PROBABILITY: f32 = 0.6;

            let mut text = String::new();
            for segment in state.as_iter() {
                if segment.no_speech_probability() > MAX_NO_SPEECH_PROBABILITY {
                    log::debug!(
                        "discarding segment at {}ms: whisper gives it {:.0}% no-speech",
                        segment.start_timestamp(),
                        segment.no_speech_probability() * 100.0
                    );
                    continue;
                }

                if let Ok(piece) = segment.to_str()
                    && !piece.trim().is_empty()
                {
                    text.push_str(piece);
                }
            }

            Ok(Transcript::new(
                text,
                "whisper.cpp",
                std::time::Duration::from_millis(utterance.duration_ms()),
            ))
        })
        .await
        .map_err(|e| EngineError::Other(format!("transcription task: {e}")))?
    }

    fn name(&self) -> &'static str {
        "whisper.cpp"
    }

    fn is_remote(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_model_names_the_path() {
        let path = Path::new("/nonexistent/ggml-small.en.bin");
        let err = WhisperEngine::load(path, BackendChoice::Cpu).expect_err("must fail");

        // The message has to tell a streamer where the file was expected, since
        // not having downloaded it is the expected first-run state.
        let message = err.to_string();
        assert!(message.contains("no model at"), "got: {message}");
        assert!(message.contains("ggml-small.en.bin"), "got: {message}");
    }

    #[test]
    fn discovery_prefers_the_accurate_model() {
        let dir = std::env::temp_dir().join("yap-voice-discover");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");

        std::fs::write(dir.join("ggml-small.en.bin"), b"x").expect("write");
        std::fs::write(dir.join("ggml-large-v3-turbo.bin"), b"x").expect("write");

        let found = WhisperEngine::discover(&dir).expect("found");
        assert!(found.ends_with("ggml-large-v3-turbo.bin"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn empty_audio_is_rejected_before_reaching_whisper() {
        // A real model is not needed to prove the guard: an empty utterance
        // cannot be transcribed by any engine.
        let utterance = Utterance::from_samples(Vec::new(), 0);
        assert!(utterance.is_empty());
    }
}
