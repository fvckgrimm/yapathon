//! Listening: capture audio, find utterances, recognise them, count the words.
//!
//! The pipeline is deliberately one-directional and each stage owns exactly one
//! decision. [`segmenter::Segmenter`] decides where an utterance ends, the
//! engine decides what was said, and [`yap_core::session::Session`] decides what
//! it is worth. Nothing downstream can influence anything upstream, which is
//! what keeps the count reproducible.

pub mod capture;
pub mod cloud;
pub mod config;
pub mod local;
pub mod pipeline;
pub mod segmenter;

pub use capture::{InputDevice, Microphone, ProbeVerdict, Resampler, classify_probe_level};
pub use cloud::GroqEngine;
pub use config::{BackendChoice, DetectedBackend};
pub use local::WhisperEngine;
pub use pipeline::{
    CollectingSink, FakeEngine, Listener, ListenerHandle, ListenerStats, WordSink, recognise_all,
};
pub use segmenter::{SegmentedUtterance, Segmenter};

use yap_core::engine::TranscriptionEngine;

/// Pick an engine from settings.
///
/// # Errors
/// Returns [`yap_core::engine::EngineError`] when the model is missing or the
/// key is absent. Both messages name what to do about it.
pub async fn build_engine(
    choice: &yap_core::settings::EngineChoice,
) -> Result<Box<dyn TranscriptionEngine>, yap_core::engine::EngineError> {
    use yap_core::settings::EngineChoice;

    match choice {
        EngineChoice::Local {
            model_path,
            backend,
        } => {
            let backend = match backend {
                yap_core::settings::LocalBackend::Auto => config::BackendChoice::Auto,
                yap_core::settings::LocalBackend::Metal => config::BackendChoice::Metal,
                yap_core::settings::LocalBackend::Cuda => config::BackendChoice::Cuda,
                yap_core::settings::LocalBackend::Vulkan => config::BackendChoice::Vulkan,
                yap_core::settings::LocalBackend::Cpu => config::BackendChoice::Cpu,
            };

            Ok(Box::new(local::WhisperEngine::load(model_path, backend)?))
        }
        EngineChoice::Groq { credential } => {
            Ok(Box::new(cloud::GroqEngine::from_keyring(credential)?))
        }
    }
}
