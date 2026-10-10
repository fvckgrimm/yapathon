//! The shared runtime state: one session, one listener, one event feed.
//!
//! Held behind an `RwLock` because everything touches it: the recognition task
//! adds utterances, the Twitch task moves the goal, the dock reads a snapshot,
//! and the overlay subscribes to changes. Each of those is short, so a lock
//! beats a message-passing design here — and the alternative, an actor with a
//! command queue, would put a round trip between every word spoken and the
//! number on screen.
//!
//! What must not happen is two writers racing the goal. Word deltas and events
//! both mutate the session, so both take the write lock and the goal math stays
//! serialised. That is the same guarantee the pipeline gives for ordering, one
//! level up.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{RwLock, broadcast};
use yap_core::goal::Event;
use yap_core::session::{Session, SessionConfig, SessionSnapshot, SessionStatus};

/// How many updates to buffer for a slow overlay client.
///
/// Small. The overlay renders the latest state, not a history, so a client that
/// falls behind wants the newest value rather than a backlog it will skip
/// through anyway.
const BROADCAST_CAPACITY: usize = 16;

/// No yapathon is running.
///
/// Its own error rather than a string, so the API can answer 409 ("your request
/// was fine, the app's state is not") instead of 500, which would read as a bug.
#[derive(Debug, thiserror::Error)]
#[error("no yapathon is running")]
pub struct NoSession;

/// Everything the app owns.
pub struct AppState {
    session: RwLock<Option<Session>>,
    /// Bumped on every change so a UI can tell whether a redraw is needed.
    revision: std::sync::atomic::AtomicU64,
    updates: broadcast::Sender<SessionSnapshot>,
    /// The last events, for the dock's feed.
    recent_events: RwLock<Vec<Event>>,
    /// Whether a microphone is currently being listened to.
    listening: RwLock<bool>,
    /// Warnings worth showing but not worth stopping for.
    warnings: RwLock<Vec<String>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        let (updates, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            session: RwLock::new(None),
            revision: std::sync::atomic::AtomicU64::new(0),
            updates,
            recent_events: RwLock::new(Vec::new()),
            listening: RwLock::new(false),
            warnings: RwLock::new(Vec::new()),
        }
    }

    /// Wrap in an `Arc` for sharing across tasks.
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Subscribe to state changes.
    ///
    /// Subscribing before the first snapshot means a client never misses the
    /// state it needs to render.
    pub fn subscribe(&self) -> broadcast::Receiver<SessionSnapshot> {
        self.updates.subscribe()
    }

    /// Current revision. Changes on every mutation.
    pub fn revision(&self) -> u64 {
        self.revision.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn bump(&self) {
        self.revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Begin a new yapathon, replacing any current one.
    pub async fn start_session(&self, config: SessionConfig) -> Result<SessionSnapshot> {
        let mut session = Session::new(config);
        session.start();

        let snapshot = {
            let mut guard = self.session.write().await;
            *guard = Some(session);
            guard.as_ref().expect("just set").snapshot()
        };

        self.recent_events.write().await.clear();
        self.warnings.write().await.clear();
        self.bump();
        let _ = self.updates.send(snapshot.clone());

        Ok(snapshot)
    }

    /// Count one recognised utterance.
    ///
    /// Inert when there is no session or it is not running, which is what makes
    /// the recogniser safe to leave running across a pause.
    pub async fn record_words(&self, text: &str, started_ms: u64, ended_ms: u64) -> u32 {
        let (words, snapshot) = {
            let mut guard = self.session.write().await;
            let Some(session) = guard.as_mut() else {
                return 0;
            };

            let words = session.record(text, started_ms, ended_ms);
            if words == 0 {
                return 0;
            }
            (words, session.snapshot())
        };

        self.bump();
        let _ = self.updates.send(snapshot);
        words
    }

    /// Apply a platform or chat event.
    pub async fn apply_event(&self, event: Event) -> u32 {
        let (granted, snapshot) = {
            let mut guard = self.session.write().await;
            let Some(session) = guard.as_mut() else {
                return 0;
            };

            let granted = session.apply_event(&event);
            self.recent_events.write().await.push(event.clone());
            (granted, session.snapshot())
        };

        self.bump();
        let _ = self.updates.send(snapshot);
        granted
    }

    /// Pause counting, without ending the run.
    pub async fn pause(&self) -> Result<SessionSnapshot> {
        self.mutate(|s| s.pause()).await
    }

    pub async fn resume(&self) -> Result<SessionSnapshot> {
        self.mutate(|s| s.resume()).await
    }

    pub async fn end(&self) -> Result<SessionSnapshot> {
        self.mutate(|s| s.end()).await
    }

    /// Void the most recent utterance.
    pub async fn undo_last(&self) -> Result<Option<u32>> {
        let (removed, snapshot) = {
            let mut guard = self.session.write().await;
            let Some(session) = guard.as_mut() else {
                return Ok(None);
            };

            let removed = session.undo_last();
            (removed, session.snapshot())
        };

        if removed.is_some() {
            self.bump();
            let _ = self.updates.send(snapshot);
        }
        Ok(removed)
    }

    /// A manual correction in words, positive or negative.
    pub async fn adjust(&self, words: i64, note: &str) -> Result<SessionSnapshot> {
        self.mutate(move |s| {
            s.adjust(words, note);
        })
        .await
    }

    /// Apply a session mutation and publish the result.
    async fn mutate(&self, f: impl FnOnce(&mut Session)) -> Result<SessionSnapshot> {
        let snapshot = {
            let mut guard = self.session.write().await;
            let session = guard.as_mut().ok_or(NoSession)?;
            f(session);
            session.snapshot()
        };

        self.bump();
        let _ = self.updates.send(snapshot.clone());
        Ok(snapshot)
    }

    /// The current snapshot, or `None` with no session.
    pub async fn snapshot(&self) -> Option<SessionSnapshot> {
        self.session.read().await.as_ref().map(|s| s.snapshot())
    }

    pub async fn status(&self) -> Option<SessionStatus> {
        self.session.read().await.as_ref().map(|s| s.status())
    }

    /// Every utterance's text and tokens, for the audit view.
    pub async fn transcript(&self) -> Vec<(u64, u32, String, bool)> {
        let guard = self.session.read().await;
        let Some(session) = guard.as_ref() else {
            return Vec::new();
        };

        session
            .utterances()
            .iter()
            .map(|u| (u.started_ms, u.words, u.text.clone(), u.voided))
            .collect()
    }

    /// Recent events, newest last.
    pub async fn events(&self) -> Vec<Event> {
        self.recent_events.read().await.clone()
    }

    pub async fn set_listening(&self, listening: bool) {
        *self.listening.write().await = listening;
        self.bump();
    }

    pub async fn is_listening(&self) -> bool {
        *self.listening.read().await
    }

    /// Record a warning shown in the dock.
    ///
    /// A failed utterance or a dropped socket is not fatal, but it should be
    /// visible: silently ignoring them makes a wrong number look authoritative.
    pub async fn warn(&self, message: impl Into<String>) {
        let message = message.into();
        log::warn!("{message}");

        let mut warnings = self.warnings.write().await;
        // Bounded: a long stream can fail a lot, and the dock only shows the
        // recent ones.
        if warnings.len() >= 50 {
            warnings.remove(0);
        }
        warnings.push(message);
        self.bump();
    }

    pub async fn warnings(&self) -> Vec<String> {
        self.warnings.read().await.clone()
    }

    /// The number of words a text would count, for a preview.
    pub async fn count_preview(
        &self,
        text: &str,
        rules: &yap_core::counting::CountingRules,
    ) -> u32 {
        yap_core::counting::count(text, rules).words
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yap_core::goal::{GoalMode, HardLimit};

    fn config() -> SessionConfig {
        SessionConfig::new(GoalMode::Fixed { words: 100 }, None)
    }

    #[tokio::test]
    async fn a_snapshot_is_published_on_every_change() {
        let state = AppState::new();
        let mut updates = state.subscribe();

        state.start_session(config()).await.expect("started");

        let snapshot = updates.recv().await.expect("first update");
        assert_eq!(snapshot.words, 0);

        state.record_words("one two three", 0, 1000).await;

        let snapshot = updates.recv().await.expect("second update");
        assert_eq!(snapshot.words, 3);
        assert!(state.revision() > 0);
    }

    #[tokio::test]
    async fn words_before_a_session_are_ignored() {
        // The recogniser may still be draining its buffer when a session ends.
        let state = AppState::new();
        assert_eq!(state.record_words("leaked words", 0, 1000).await, 0);
    }

    #[tokio::test]
    async fn events_before_a_session_are_ignored() {
        let state = AppState::new();
        let event = Event::granting("sub", 100);
        assert_eq!(state.apply_event(event).await, 0);
    }

    #[tokio::test]
    async fn a_subscription_moves_the_goal() {
        let state = AppState::new();
        state
            .start_session(SessionConfig::new(
                GoalMode::Extend { base_words: 500 },
                Some(HardLimit::words(1000)),
            ))
            .await
            .expect("started");

        let granted = state.apply_event(Event::granting("sub", 250)).await;
        assert_eq!(granted, 250);

        let snapshot = state.snapshot().await.expect("snapshot");
        assert_eq!(snapshot.target, 750);

        // Chat is recorded for the dock's feed.
        let events = state.events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "sub");
    }

    #[tokio::test]
    async fn the_hard_limit_ends_the_run() {
        let state = AppState::new();
        state
            .start_session(SessionConfig::new(
                GoalMode::Extend { base_words: 500 },
                Some(HardLimit::words(600)),
            ))
            .await
            .expect("started");

        state.apply_event(Event::granting("sub", 400)).await;

        let snapshot = state.snapshot().await.expect("snapshot");
        assert_eq!(snapshot.target, 600);
        assert_eq!(snapshot.status, SessionStatus::Capped);
        assert!(snapshot.capped);
    }

    #[tokio::test]
    async fn pausing_stops_the_count() {
        let state = AppState::new();
        state.start_session(config()).await.expect("started");

        state.record_words("one two", 0, 1000).await;
        state.pause().await.expect("paused");

        // The recogniser draining after the pause must not leak words in.
        assert_eq!(state.record_words("three four", 1000, 2000).await, 0);

        let snapshot = state.snapshot().await.expect("snapshot");
        assert_eq!(snapshot.words, 2);
        assert_eq!(snapshot.status, SessionStatus::Paused);

        state.resume().await.expect("resumed");
        state.record_words("three", 2000, 3000).await;
        assert_eq!(state.snapshot().await.expect("snapshot").words, 3);
    }

    #[tokio::test]
    async fn undo_publishes_a_smaller_total() {
        let state = AppState::new();
        state.start_session(config()).await.expect("started");

        state.record_words("one two three", 0, 1000).await;
        state.record_words("four five", 1000, 2000).await;

        let mut updates = state.subscribe();
        let removed = state.undo_last().await.expect("undone");
        assert_eq!(removed, Some(2));

        let snapshot = updates.recv().await.expect("update");
        assert_eq!(snapshot.words, 3);
    }

    #[tokio::test]
    async fn the_transcript_is_available_for_audit() {
        // When chat disputes the count, the streamer needs to be able to show
        // what was actually heard.
        let state = AppState::new();
        state.start_session(config()).await.expect("started");

        state.record_words("one two three", 0, 1000).await;
        state.record_words("four five", 1000, 2000).await;
        state.undo_last().await.expect("undone");

        let transcript = state.transcript().await;
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].1, 3);
        assert!(!transcript[0].3);
        // The voided entry stays, marked.
        assert!(transcript[1].3);
    }

    #[tokio::test]
    async fn adjustments_are_published() {
        let state = AppState::new();
        state.start_session(config()).await.expect("started");
        state.record_words("one two", 0, 1000).await;

        state
            .adjust(50, "missed a stretch")
            .await
            .expect("adjusted");
        assert_eq!(state.snapshot().await.expect("snapshot").words, 52);

        state.adjust(-20, "overcounted").await.expect("adjusted");
        assert_eq!(state.snapshot().await.expect("snapshot").words, 32);
    }

    #[tokio::test]
    async fn warnings_are_recorded_and_bounded() {
        let state = AppState::new();
        for i in 0..60 {
            state.warn(format!("failed utterance {i}")).await;
        }

        let warnings = state.warnings().await;
        assert_eq!(warnings.len(), 50, "the dock should not grow without bound");
        assert!(warnings.last().expect("last").contains("59"));
    }

    #[tokio::test]
    async fn mutating_without_a_session_is_an_error() {
        let state = AppState::new();
        assert!(state.pause().await.is_err());
        assert!(state.end().await.is_err());
    }

    #[tokio::test]
    async fn starting_a_new_session_clears_the_old_one() {
        let state = AppState::new();
        state.start_session(config()).await.expect("started");
        state.record_words("one two three", 0, 1000).await;
        state.apply_event(Event::granting("sub", 100)).await;

        state.start_session(config()).await.expect("restarted");

        let snapshot = state.snapshot().await.expect("snapshot");
        assert_eq!(snapshot.words, 0);
        assert!(state.events().await.is_empty());
    }

    #[tokio::test]
    async fn reaching_the_goal_publishes_a_win() {
        let state = AppState::new();
        state
            .start_session(SessionConfig::new(GoalMode::Fixed { words: 3 }, None))
            .await
            .expect("started");

        let mut updates = state.subscribe();
        state.record_words("one two three", 0, 1000).await;

        let snapshot = updates.recv().await.expect("update");
        assert_eq!(snapshot.status, SessionStatus::Won);
        assert!(snapshot.complete);
    }

    #[tokio::test]
    async fn listening_state_is_tracked() {
        let state = AppState::new();
        assert!(!state.is_listening().await);
        state.set_listening(true).await;
        assert!(state.is_listening().await);
    }

    #[tokio::test]
    async fn a_slow_subscriber_does_not_block_the_session() {
        // The overlay can lag or be closed. Broadcasting must never make a word
        // wait for a reader.
        let state = AppState::new();
        let mut updates = state.subscribe();
        state.start_session(config()).await.expect("started");

        // Fill the channel and never read again.
        for i in 0..40 {
            state.record_words("word", i * 1000, i * 1000 + 500).await;
        }

        // A fresh subscriber still sees the current state, not a gap.
        let fresh = state.subscribe();
        let current = state.snapshot().await.expect("snapshot");
        assert_eq!(current.words, 40);

        // And the lagging subscriber gets a lagged signal rather than blocking.
        assert!(matches!(
            updates.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))
        ));
        drop(fresh);
    }
}
