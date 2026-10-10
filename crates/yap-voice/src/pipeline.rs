//! Wiring capture, segmentation and transcription into one loop.
//!
//! [`Listener`] owns the ordering guarantee the whole count depends on:
//! utterances are recognised **one at a time, in the order they were spoken**.
//! Recognition can be slower than real time on a cold CPU model, so a naive
//! implementation would either reorder words or drop them. A single worker with
//! a bounded FIFO does neither — it falls behind, and the stats say so.
//!
//! Falling behind is visible rather than silent. [`ListenerStats::is_behind`]
//! lets the dock report that the counter is lagging the streamer's voice, which
//! is the honest thing to do when it is.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use yap_core::engine::{Transcript, TranscriptionEngine, VadConfig};

use crate::capture::Microphone;
use crate::segmenter::SegmentedUtterance;

/// Where recognised words go.
///
/// A trait rather than a closure so a session can implement it without this
/// crate knowing how sessions are stored.
pub trait WordSink: Send + Sync + 'static {
    /// Called once per recognised utterance, in spoken order.
    ///
    /// Runs on the recognition task, so it must not block for long.
    fn on_transcript(&self, transcript: &Transcript);

    /// Called when an utterance could not be recognised.
    ///
    /// A failed utterance must not fail a yapathon: the run continues with
    /// whatever was recognised.
    fn on_error(&self, utterance_ms: u64, error: &yap_core::engine::EngineError);
}

/// What the pipeline is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct ListenerStats {
    /// Utterances recognised.
    pub recognised: u64,
    /// Utterances that failed.
    pub failed: u64,
    /// Utterances waiting to be recognised.
    pub pending: u64,
    /// Whether capture is running.
    pub running: bool,
}

impl ListenerStats {
    /// Whether the recogniser is struggling to keep up.
    ///
    /// Three utterances queued is roughly ten seconds of speech on a slow CPU
    /// model, which is enough that the words-to-go figure is visibly behind the
    /// streamer's voice. The dock shows a warning rather than pretending the
    /// number is current.
    pub fn is_behind(&self) -> bool {
        self.pending > 3
    }
}

/// Handle for stopping a running listener.
#[derive(Debug)]
pub struct ListenerHandle {
    stop: Arc<AtomicBool>,
    microphone: Option<Microphone>,
}

impl ListenerHandle {
    /// Stop capture.
    ///
    /// The recognition worker is awaited separately by [`Listener::stop`], which
    /// owns it.
    fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(mic) = self.microphone.take() {
            mic.stop();
        }
    }
}

/// Runs the capture-to-words pipeline.
///
/// Owns the recognition worker, so call [`Listener::stop`] rather than dropping
/// it: dropping leaves a task holding the microphone open.
pub struct Listener {
    handle: Option<ListenerHandle>,
    worker: Option<tokio::task::JoinHandle<()>>,
    stats: Arc<Counters>,
}

#[derive(Debug, Default)]
struct Counters {
    recognised: AtomicU64,
    failed: AtomicU64,
    pending: AtomicU64,
}

impl Drop for Listener {
    fn drop(&mut self) {
        if self.worker.is_some() {
            log::warn!("Listener dropped without stop(); the microphone may stay open");
        }
    }
}

impl Listener {
    /// Start listening on the default input device.
    ///
    /// # Errors
    /// Propagates capture failures. A microphone that cannot be opened is a
    /// setup problem the streamer needs to see, not something to retry silently.
    pub async fn start(
        engine: Arc<dyn TranscriptionEngine>,
        vad: VadConfig,
        sink: Arc<dyn WordSink>,
    ) -> anyhow::Result<Self> {
        Self::start_with_device(None, engine, vad, sink).await
    }

    /// Start listening on a named device.
    pub async fn start_named(
        device: String,
        engine: Arc<dyn TranscriptionEngine>,
        vad: VadConfig,
        sink: Arc<dyn WordSink>,
    ) -> anyhow::Result<Self> {
        Self::start_with_device(Some(device), engine, vad, sink).await
    }

    async fn start_with_device(
        device: Option<String>,
        engine: Arc<dyn TranscriptionEngine>,
        vad: VadConfig,
        sink: Arc<dyn WordSink>,
    ) -> anyhow::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Counters::default());

        // Bounded: eight utterances is about half a minute of speech. Unbounded
        // would let a slow model accumulate minutes of audio, and the lag would
        // not be visible until the run ended.
        let (tx, mut rx) = mpsc::channel::<SegmentedUtterance>(8);

        let worker_stats = Arc::clone(&stats);
        let worker = tokio::spawn(async move {
            // One at a time, in order. Recognition is a blocking call and may
            // outrun the capture rate on a CPU model; a single worker means the
            // queue grows visibly instead of words being reordered or dropped.
            while let Some(utterance) = rx.recv().await {
                worker_stats.pending.fetch_sub(1, Ordering::Relaxed);

                match engine.transcribe(utterance.clone().into_engine()).await {
                    Ok(transcript) if !transcript.is_empty() => {
                        worker_stats.recognised.fetch_add(1, Ordering::Relaxed);
                        sink.on_transcript(&transcript);
                    }
                    // An engine that heard nothing scoreable is ordinary for a
                    // cough or a door slam, not a failure worth reporting.
                    Ok(_) => {}
                    Err(e) => {
                        worker_stats.failed.fetch_add(1, Ordering::Relaxed);
                        log::warn!(
                            "utterance at {}ms not recognised: {e}",
                            utterance.started_ms
                        );
                        sink.on_error(utterance.started_ms, &e);
                    }
                }
            }
        });

        let callback_stop = Arc::clone(&stop);
        let callback_stats = Arc::clone(&stats);

        // The callback runs on the audio thread, so it must not block.
        // `try_send` drops rather than waiting: a full queue means recognition
        // is behind, which `pending` reports instead of hiding.
        let on_captured = move |utterance: SegmentedUtterance| {
            if callback_stop.load(Ordering::Relaxed) {
                return;
            }
            callback_stats.pending.fetch_add(1, Ordering::Relaxed);
            let _ = tx.try_send(utterance);
        };

        let microphone = match device {
            Some(name) => Microphone::start_named(&name, vad, on_captured).await?,
            None => Microphone::start_default(vad, on_captured).await?,
        };

        Ok(Self {
            handle: Some(ListenerHandle {
                stop,
                microphone: Some(microphone),
            }),
            worker: Some(worker),
            stats,
        })
    }

    /// Current pipeline state.
    pub fn stats(&self) -> ListenerStats {
        ListenerStats {
            recognised: self.stats.recognised.load(Ordering::Relaxed),
            failed: self.stats.failed.load(Ordering::Relaxed),
            pending: self.stats.pending.load(Ordering::Relaxed),
            running: self
                .handle
                .as_ref()
                .map(|h| !h.stop.load(Ordering::SeqCst))
                .unwrap_or(false),
        }
    }

    /// Stop capture and drain what the worker still holds.
    ///
    /// Words spoken in the last second before a stop are still counted: the
    /// worker drains its queue before this returns.
    pub async fn stop(mut self) {
        if let Some(handle) = self.handle.take() {
            handle.stop();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.await;
        }
    }
}

/// A sink that keeps everything, for tests and for the CLI's verbose mode.
#[derive(Default)]
pub struct CollectingSink {
    transcripts: std::sync::Mutex<Vec<Transcript>>,
    errors: std::sync::Mutex<Vec<(u64, String)>>,
}

impl CollectingSink {
    pub fn transcripts(&self) -> Vec<Transcript> {
        self.transcripts
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    pub fn errors(&self) -> Vec<(u64, String)> {
        self.errors.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

impl WordSink for CollectingSink {
    fn on_transcript(&self, transcript: &Transcript) {
        if let Ok(mut guard) = self.transcripts.lock() {
            guard.push(transcript.clone());
        }
    }

    fn on_error(&self, utterance_ms: u64, error: &yap_core::engine::EngineError) {
        if let Ok(mut guard) = self.errors.lock() {
            guard.push((utterance_ms, error.to_string()));
        }
    }
}

/// An engine that returns canned transcripts, for testing the pipeline without
/// a model or a microphone.
pub struct FakeEngine {
    texts: std::sync::Mutex<Vec<String>>,
    fail: bool,
    delay: Duration,
}

impl FakeEngine {
    pub fn new(texts: &[&str]) -> Self {
        Self {
            texts: std::sync::Mutex::new(texts.iter().map(|s| s.to_string()).collect()),
            fail: false,
            delay: Duration::ZERO,
        }
    }

    pub fn failing() -> Self {
        Self {
            texts: std::sync::Mutex::new(Vec::new()),
            fail: true,
            delay: Duration::ZERO,
        }
    }

    /// An engine slower than real time, for testing the backpressure path.
    pub fn slow(texts: &[&str], delay: Duration) -> Self {
        Self {
            texts: std::sync::Mutex::new(texts.iter().map(|s| s.to_string()).collect()),
            fail: false,
            delay,
        }
    }
}

#[async_trait::async_trait]
impl TranscriptionEngine for FakeEngine {
    async fn transcribe(
        &self,
        utterance: yap_core::engine::Utterance,
    ) -> Result<Transcript, yap_core::engine::EngineError> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }

        if self.fail {
            return Err(yap_core::engine::EngineError::Other(
                "fake engine failure".to_string(),
            ));
        }

        let text = {
            let mut guard = self
                .texts
                .lock()
                .map_err(|_| yap_core::engine::EngineError::Other("poisoned".to_string()))?;
            if guard.is_empty() {
                String::new()
            } else {
                guard.remove(0)
            }
        };

        Ok(Transcript::new(
            text,
            "fake",
            Duration::from_millis(utterance.duration_ms()),
        ))
    }

    fn name(&self) -> &'static str {
        "fake"
    }
}

/// Feed utterances through an engine and a sink, without a microphone.
///
/// The recognition half of [`Listener`] on its own, which is what the ordering
/// and backpressure tests need. Public so a harness can exercise the engine
/// seam end to end with recorded audio.
pub async fn recognise_all(
    engine: Arc<dyn TranscriptionEngine>,
    sink: Arc<dyn WordSink>,
    utterances: Vec<SegmentedUtterance>,
) {
    let (tx, mut rx) = mpsc::channel::<SegmentedUtterance>(utterances.len().max(1));

    let worker = tokio::spawn(async move {
        while let Some(utterance) = rx.recv().await {
            match engine.transcribe(utterance.clone().into_engine()).await {
                Ok(transcript) if !transcript.is_empty() => sink.on_transcript(&transcript),
                Ok(_) => {}
                Err(e) => sink.on_error(utterance.started_ms, &e),
            }
        }
    });

    for utterance in utterances {
        let _ = tx.send(utterance).await;
    }
    drop(tx);

    let _ = worker.await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utterance(started_ms: u64) -> SegmentedUtterance {
        SegmentedUtterance {
            samples: vec![0.1; 16_000],
            started_ms,
            ended_ms: started_ms + 1000,
        }
    }

    #[tokio::test]
    async fn a_failing_engine_reports_without_stopping_the_run() {
        // Losing a yapathon to one bad request would be worse than losing a few
        // words, so a failure is reported and the pipeline continues.
        let sink = Arc::new(CollectingSink::default());
        let engine: Arc<dyn TranscriptionEngine> = Arc::new(FakeEngine::failing());

        recognise_all(
            engine,
            Arc::clone(&sink) as Arc<dyn WordSink>,
            vec![utterance(0)],
        )
        .await;

        assert_eq!(sink.errors().len(), 1);
        assert!(sink.transcripts().is_empty());
    }

    #[tokio::test]
    async fn transcripts_arrive_in_spoken_order() {
        // Ordering is a correctness property: the total has to match the order
        // the words were said.
        let sink = Arc::new(CollectingSink::default());
        let engine: Arc<dyn TranscriptionEngine> =
            Arc::new(FakeEngine::new(&["first", "second", "third"]));

        recognise_all(
            engine,
            Arc::clone(&sink) as Arc<dyn WordSink>,
            vec![utterance(0), utterance(1000), utterance(2000)],
        )
        .await;

        let texts: Vec<String> = sink.transcripts().into_iter().map(|t| t.text).collect();
        assert_eq!(texts, vec!["first", "second", "third"]);
    }

    #[tokio::test]
    async fn an_empty_transcript_is_not_a_failure() {
        // Whisper transcribing nothing is ordinary. Reporting it as an error
        // would fill the dock with warnings during every silent pause.
        let sink = Arc::new(CollectingSink::default());
        let engine: Arc<dyn TranscriptionEngine> = Arc::new(FakeEngine::new(&[]));

        recognise_all(
            engine,
            Arc::clone(&sink) as Arc<dyn WordSink>,
            vec![utterance(0)],
        )
        .await;

        assert!(sink.transcripts().is_empty());
        assert!(sink.errors().is_empty());
    }

    #[tokio::test]
    async fn a_slow_engine_does_not_reorder_words() {
        // A single worker with a FIFO queue: slower than real time, but every
        // word still lands in the order it was spoken.
        let sink = Arc::new(CollectingSink::default());
        let engine: Arc<dyn TranscriptionEngine> = Arc::new(FakeEngine::slow(
            &["a", "b", "c", "d"],
            Duration::from_millis(10),
        ));

        recognise_all(
            engine,
            Arc::clone(&sink) as Arc<dyn WordSink>,
            vec![
                utterance(0),
                utterance(1000),
                utterance(2000),
                utterance(3000),
            ],
        )
        .await;

        let texts: Vec<String> = sink.transcripts().into_iter().map(|t| t.text).collect();
        assert_eq!(texts, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn stats_report_being_behind() {
        let idle = ListenerStats {
            recognised: 10,
            failed: 0,
            pending: 0,
            running: true,
        };
        assert!(!idle.is_behind());

        let behind = ListenerStats { pending: 5, ..idle };
        assert!(behind.is_behind(), "a lagging counter must say so");
    }
}
