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
    /// Whether the "events are arriving but there is nothing to extend" note
    /// has already been said for this stretch.
    ///
    /// Said once rather than per event: chat can be busy, and a line per cheer
    /// would bury the whisper loading output it is trying to be noticed
    /// against.
    idle_event_note: std::sync::atomic::AtomicBool,
    /// Which overlay palette to serve.
    ///
    /// Held here rather than read from settings at request time because the
    /// overlay must render with no filesystem access on the hot path, and because
    /// `?theme=` needs a fallback to compare against.
    ///
    /// Stored as written rather than resolved, so a combined name from an older
    /// settings file is still recognisable when the request comes in and can be
    /// mapped to a pair — resolving here would throw away which axis it named.
    overlay_theme: RwLock<String>,

    /// Which overlay shape to serve.
    overlay_style: RwLock<String>,

    /// Told when the recognition settings change, so the microphone can reload.
    ///
    /// Here rather than on the app's hub because the settings panel does not have
    /// a hub — it has the session entity, and that is what holds this. Every
    /// other piece of shared state already goes through `AppState` for the same
    /// reason.
    ///
    /// Optional so the many tests that build a bare `AppState` do not have to
    /// care: `None` means nobody is listening, which is not an error.
    capture_changes: std::sync::OnceLock<Option<tokio::sync::watch::Sender<CaptureNotice>>>,
}

/// "Something about the microphone changed."
///
/// A unit type rather than the configuration itself, deliberately: the
/// microphone thread is what has to work out *what* changed and decide whether
/// to rebuild, and it holds the configuration that is currently running. Sending
/// the value here would put a second copy in play for no reader but the one that
/// already has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureNotice;

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        let (updates, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            idle_event_note: std::sync::atomic::AtomicBool::new(false),
            session: RwLock::new(None),
            revision: std::sync::atomic::AtomicU64::new(0),
            updates,
            recent_events: RwLock::new(Vec::new()),
            listening: RwLock::new(false),
            warnings: RwLock::new(Vec::new()),
            overlay_theme: RwLock::new(yap_core::settings::default_overlay_theme()),
            overlay_style: RwLock::new(yap_core::settings::default_overlay_style()),
            capture_changes: std::sync::OnceLock::new(),
        }
    }

    /// The overlay look to serve, given `override` if the request asked for one.
    ///
    /// An unknown name falls back rather than failing: the id can come out of a
    /// hand-edited settings file or a mistyped query, and the overlay is the one
    /// thing that has to work during a stream.
    ///
    /// Returns `(palette, style)`, because a request may pin one axis and leave
    /// the other to the setting — `?theme=neon` should keep whatever shape the
    /// streamer chose in the app rather than resetting it to the default.
    pub fn overlay_look(&self, theme: Option<&str>, style: Option<&str>) -> (String, String) {
        let setting = self
            .overlay_theme
            .try_read()
            .map(|t| t.clone())
            .unwrap_or_default();

        // A combined name from an older settings file resolves to the pair that
        // reproduces what it used to show, so an upgrade cannot silently change
        // someone's overlay mid-stream.
        let requested = theme.map(str::to_string).unwrap_or_else(|| setting.clone());

        // Only the *stored* name can be a legacy one. A `?theme=` in a URL is
        // typed by someone standing in front of the app right now, and `card` in
        // a URL means "the card palette" — not "the old combined card theme",
        // which would then claim the shape and ignore whatever the streamer has
        // set in the app.
        let stored_is_legacy = theme.is_none() && crate::overlay::legacy_look(&setting).is_some();

        let (palette, legacy_style) = match crate::overlay::legacy_look(&requested) {
            Some(pair) => (pair.0.to_string(), Some(pair.1.to_string())),
            None => (
                crate::overlay::overlay_palette(&requested).id.to_string(),
                None,
            ),
        };

        let stored_style = self
            .overlay_style
            .try_read()
            .map(|s| s.clone())
            .unwrap_or_default();

        // An absent `?style=` keeps whatever the app is set to — but *only* when the
        // app has actually been set to one, or when the file predates the split.
        //
        // A settings file written before the two axes existed has no
        // `overlay_style` key at all, so it deserialises to the default
        // `"corner"` — a real value, not an absence, which would silently
        // overwrite the shape the legacy mapping just recovered. Someone whose
        // file says `overlay_theme = "strip"` would wake up to a bottom-right
        // card.
        let default_style = match legacy_style {
            Some(style) if stored_is_legacy => style,
            _ if stored_style.is_empty() => crate::overlay::DEFAULT_OVERLAY_STYLE.to_string(),
            _ => crate::overlay::overlay_style(&stored_style).id.to_string(),
        };

        let style = style.map_or(default_style, |requested| {
            crate::overlay::overlay_style(requested).id.to_string()
        });

        (palette, style)
    }

    /// The palette to serve, given `override` if the request asked for one.
    pub fn overlay_theme(&self, requested: Option<&str>) -> String {
        self.overlay_look(requested, None).0
    }

    /// The style to serve, given `override` if the request asked for one.
    pub fn overlay_style(&self, requested: Option<&str>) -> String {
        self.overlay_look(None, requested).1
    }

    /// Set the palette served when the request does not ask for one.
    ///
    /// Stored verbatim rather than resolved, so an old combined name survives
    /// to be mapped per request.
    pub fn set_overlay_theme(&self, theme: &str) {
        if let Ok(mut guard) = self.overlay_theme.try_write() {
            *guard = theme.trim().to_lowercase();
        }
    }

    /// Announce that the recognition settings changed.
    ///
    /// A no-op until something calls [`AppState::watch_capture`], which is what
    /// the microphone thread does at startup. Sending before that would fail for
    /// want of a receiver, and there is nothing to tell.
    pub fn notify_capture_changed(&self) {
        if let Some(Some(sender)) = self.capture_changes.get() {
            let _ = sender.send(CaptureNotice);
        }
    }

    /// Watch for recognition settings changing.
    ///
    /// The returned receiver only sees *later* notifications, so a caller that
    /// needs to know the starting point reads its own configuration for that.
    /// Which is why this carries no value.
    pub fn watch_capture(&self) -> Option<tokio::sync::watch::Receiver<CaptureNotice>> {
        let sender = self
            .capture_changes
            .get_or_init(|| {
                let (sender, receiver) = tokio::sync::watch::channel(CaptureNotice);
                // The first receiver is dropped on purpose: it exists only to
                // create the channel, and holding it would stop `send` from
                // reporting "no receivers", which is not a condition worth
                // branching on.
                drop(receiver);
                Some(sender)
            })
            .clone()?;

        Some(sender.subscribe())
    }

    /// Set the shape served when the request does not ask for one.
    pub fn set_overlay_style(&self, style: &str) {
        if let Ok(mut guard) = self.overlay_style.try_write() {
            *guard = style.trim().to_lowercase();
        }
    }

    /// Publish the current snapshot once a second, for as long as this lives.
    ///
    /// ## Why this exists
    ///
    /// Snapshots are published on mutation, and every mutation is something
    /// somebody *did* — a word was counted, a sub arrived, the run was paused.
    /// Elapsed time is not something anybody did. It is `started.elapsed()`,
    /// read fresh whenever a snapshot is built, so it changes every second
    /// whether or not anything else does.
    ///
    /// With nothing publishing on a clock, a running yapathon showed a frozen
    /// timer on the overlay and in the window, and a frozen words-per-minute,
    /// and updated only when the streamer happened to say something. Chat and
    /// counts moved, so it looked like a rendering bug rather than a missing
    /// heartbeat — and refreshing "fixed" it, which is the worst possible
    /// symptom, because it tells you the data is real.
    ///
    /// Once a second is the interval because that is the resolution of anything
    /// displayed: the overlay shows `m:ss`, so a faster tick would publish
    /// snapshots nobody can tell apart. `apply` in the window compares
    /// snapshots before repainting, so a tick that lands on the same second
    /// costs nothing.
    pub fn start_ticker(self: &Arc<Self>) {
        let state = Arc::clone(self);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));

            // The first tick fires immediately; the session may not exist yet and
            // the tick is a no-op then anyway.
            loop {
                ticker.tick().await;

                // No session means nothing to say, and no session is the state
                // for most of the app's life.
                let Some(snapshot) = state.snapshot().await else {
                    continue;
                };

                // Not bumped: nothing changed, this is a clock. The revision is
                // how a consumer tells a real edit from a re-read.
                let _ = state.updates.send(snapshot);
            }
        });
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
                // Chat *is* working, and the events are being thrown away.
                // Saying so once is the difference between "this does not work"
                // and "start a yapathon first", which is otherwise a question
                // nobody can answer from the screen.
                if !self
                    .idle_event_note
                    .swap(true, std::sync::atomic::Ordering::SeqCst)
                {
                    log::info!(
                        "{} {} received, but no yapathon is running — start one and it will count",
                        event.platform,
                        event.kind
                    );
                }
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
    async fn a_running_session_publishes_a_moving_clock() {
        // The symptom this fixes: the count and the chat updated but the timer,
        // the words-per-minute and "to go" sat frozen, and only a manual refresh
        // moved them.
        //
        // Snapshots used to be published on mutation only. Elapsed time is
        // `started.elapsed()` read at snapshot time, so it changes every second
        // without anybody doing anything — and with nothing publishing, a silent
        // yapathon looked identical to a broken one. Chat still moved, which is
        // what made it read as a rendering bug rather than a missing heartbeat.
        let state = AppState::shared();
        let mut updates = state.subscribe();

        state.start_session(config()).await.expect("start");
        state.start_ticker();

        // Drain what has already been published. `recv` hands back buffered
        // messages instantly, so without this the two "consecutive" snapshots
        // are the first two ticks from startup and the test measures the tick
        // phase rather than the clock.
        while updates.try_recv().is_ok() {}

        let first = updates.recv().await.expect("a tick");
        let second = updates.recv().await.expect("another tick");

        assert!(
            second.elapsed_secs > first.elapsed_secs,
            "elapsed did not move: {} then {}",
            first.elapsed_secs,
            second.elapsed_secs,
        );
        assert_eq!(
            first.words, second.words,
            "nothing was said, so the count must be the same — only the clock moves",
        );
    }

    #[tokio::test]
    async fn the_ticker_is_quiet_when_there_is_no_session() {
        // Most of the app's life has no run going. A ticker that published
        // forever would repaint the window forever for nothing.
        let state = AppState::shared();
        let mut updates = state.subscribe();

        state.start_ticker();
        tokio::time::sleep(std::time::Duration::from_millis(1_250)).await;

        assert!(
            updates.try_recv().is_err(),
            "a tick with no session should publish nothing",
        );
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
