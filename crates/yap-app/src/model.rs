//! The GPUI entity that owns a yapathon's view of the world.
//!
//! One [`Entity<SessionModel>`] owns every piece of state the window reads:
//! the snapshot, the transcript, the event feed, the warnings, and the
//! microphone handle. Nothing is stored in the view, because a view is
//! re-rendered and a control's state that lives in a closure is a control whose
//! state is rebuilt from nothing on every redraw.
//!
//! Updates arrive from a tokio task over an [`async_channel`]. That is the seam
//! that matters: recognition runs on the runtime, the window is owned by GPUI's
//! executor, and neither can await the other directly. An async channel has no
//! reactor requirement on the receiving side, so a `tokio::sync::mpsc` would
//! panic there for the same reason `reqwest` does without a runtime.

use std::sync::Arc;

// Explicit imports rather than `use gpui_kit::*`.
//
// The glob re-exports the whole gpui surface, and a `#[cfg(test)]` module that
// does `use super::*` then re-imports all of it a second time. That expansion
// is deep enough to hit the compiler's recursion limit while expanding
// `#[test]`, which surfaces as an unrelated-looking error inside the test.
use gpui_kit::{AppContext as _, Entity};
use yap_core::goal::{Event, GoalMode};
use yap_core::session::{SessionSnapshot, SessionStatus};
use yap_server::AppState;

/// A message from the background work to the UI.
#[derive(Debug)]
pub enum ModelMessage {
    /// The session changed.
    State(SessionSnapshot),
    /// The transcript changed.
    Transcript(Vec<TranscriptLine>),
    /// An event was received.
    Event(Event),
    /// Something worth saying, not worth stopping for.
    Warning(String),
    /// Capture started or stopped.
    Listening(bool),
    /// Recognition is falling behind.
    Behind(bool),
}

/// One line of the audit trail.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptLine {
    pub words: u32,
    pub text: String,
    pub voided: bool,
}

/// Everything the window renders.
#[derive(Debug, Clone, Default)]
pub struct SessionModel {
    pub snapshot: Option<SessionSnapshot>,
    pub transcript: Vec<TranscriptLine>,
    pub events: Vec<Event>,
    pub warnings: Vec<String>,
    pub listening: bool,
    /// The recognition queue is backing up; the counter is behind the voice.
    pub behind: bool,
    /// Last microphone error, shown in place of the controls.
    pub mic_error: Option<String>,
    /// The base URL OBS's browser source should point at, without a query.
    pub overlay_url: String,
    /// Which overlay theme is in force.
    ///
    /// Carried on the model because it has to reach two places — the URL the Run
    /// tab offers to copy, and the server that serves it — and because the view
    /// already observes this entity, so a change repaints the window without
    /// anything else having to notice.
    pub overlay_theme: String,
}

impl SessionModel {
    /// What to show for the headline number.
    ///
    /// A time run shows a clock, not a word total: what the streamer watches
    /// is how long is left, not how much they have said.
    pub fn headline(&self) -> String {
        let Some(snapshot) = &self.snapshot else {
            return "0".to_string();
        };

        match &snapshot.mode {
            GoalMode::Time { .. } => {
                let remaining = snapshot.target.saturating_sub(snapshot.achieved);
                format_duration(remaining)
            }
            _ => snapshot.words.to_string(),
        }
    }

    /// The unit shown beside the headline.
    pub fn headline_unit(&self) -> &'static str {
        match self.snapshot.as_ref().map(|s| &s.mode) {
            Some(GoalMode::Time { .. }) => "left",
            _ => "words",
        }
    }

    /// The `words / target` line, or the equivalent for a time run.
    pub fn goal_line(&self) -> String {
        let Some(snapshot) = &self.snapshot else {
            return "no yapathon".to_string();
        };

        match &snapshot.mode {
            GoalMode::Time { .. } => format!("{} elapsed", format_duration(snapshot.achieved)),
            _ => format!("of {} words", snapshot.target),
        }
    }

    /// Progress in the range 0.0..=1.0.
    pub fn fraction(&self) -> f32 {
        self.snapshot.as_ref().map(|s| s.fraction).unwrap_or(0.0)
    }

    /// Words left to say, or seconds of clock left.
    pub fn remaining(&self) -> String {
        let Some(snapshot) = &self.snapshot else {
            return "—".to_string();
        };

        match &snapshot.mode {
            GoalMode::Time { .. } => {
                format_duration(snapshot.target.saturating_sub(snapshot.achieved))
            }
            _ => format!("{} to go", snapshot.remaining),
        }
    }

    pub fn rate(&self) -> String {
        match self.snapshot.as_ref().and_then(|s| s.words_per_minute) {
            Some(rate) => format!("{} wpm", rate.round() as i32),
            None => "— wpm".to_string(),
        }
    }

    pub fn elapsed(&self) -> String {
        match &self.snapshot {
            Some(snapshot) => format_duration(snapshot.achieved),
            None => "0:00".to_string(),
        }
    }

    /// Score and bonuses from chat.
    pub fn from_chat(&self) -> String {
        match &self.snapshot {
            Some(snapshot) => format!("{}", snapshot.score + snapshot.bonus_total),
            None => "0".to_string(),
        }
    }

    pub fn fillers(&self) -> String {
        match &self.snapshot {
            Some(snapshot) => snapshot.fillers.to_string(),
            None => "0".to_string(),
        }
    }

    pub fn status_label(&self) -> &'static str {
        self.snapshot
            .as_ref()
            .map(|s| s.status.label())
            .unwrap_or("no yapathon")
    }

    /// Whether a button should be enabled.
    ///
    /// Derived from the status rather than tracked separately: two pieces of
    /// state that must agree will eventually not.
    pub fn can_start(&self) -> bool {
        !self.snapshot.as_ref().is_some_and(|s| s.status.is_active())
    }

    pub fn can_pause(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| s.status == SessionStatus::Running)
    }

    pub fn can_resume(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| s.status == SessionStatus::Paused)
    }

    /// Undo is available once anything has been counted.
    pub fn can_undo(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.utterances > 0)
    }

    /// Whether the hard limit has been reached.
    pub fn is_capped(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.capped)
    }

    /// Apply a message.
    /// Fold a message into the model.
    ///
    /// Returns whether anything actually changed, which is the caller's cue to
    /// repaint.
    ///
    /// ## Why this returns a bool at all
    ///
    /// The obvious version mutates and returns nothing, and it is wrong in the
    /// worst possible way: the entity's state is correct, every read is correct, and
    /// nothing tells the window to redraw. The counter sits at zero through an
    /// entire yapathon. `SessionEntity::new` subscribes the view to this entity, so
    /// the notification is the entire mechanism — a mutation without one is a change
    /// nobody can see.
    ///
    /// The `bool` exists so the caller can skip the notification for a message that
    /// changed nothing, which matters because a running session pushes a snapshot
    /// several times a second and every one of them would otherwise be a repaint.
    pub fn apply(&mut self, message: ModelMessage) -> bool {
        match message {
            ModelMessage::State(snapshot) => {
                // Compared rather than assigned, because a session pushes a
                // snapshot on every mutation *and* on every tick of the elapsed
                // clock. Notifying on all of them would repaint the window a few
                // times a second for no visible change, which on a slow GPU is
                // the difference between a live counter and a stuttering one.
                //
                // `PartialEq` on the snapshot is what makes "did this change?"
                // answerable at all; without it the only options are never
                // repainting or repainting constantly.
                if self.snapshot.as_ref() == Some(&snapshot) {
                    return false;
                }

                self.snapshot = Some(snapshot);
                true
            }
            ModelMessage::Transcript(lines) => {
                if self.transcript == lines {
                    return false;
                }

                self.transcript = lines;
                true
            }
            ModelMessage::Event(event) => {
                self.events.push(event);
                // Bounded: a long stream can produce thousands of events and
                // the list only ever shows the recent ones.
                if self.events.len() > 200 {
                    self.events.remove(0);
                }
                true
            }
            ModelMessage::Warning(warning) => {
                self.warnings.push(warning);
                if self.warnings.len() > 50 {
                    self.warnings.remove(0);
                }
                true
            }
            ModelMessage::Listening(listening) => {
                let changed = self.listening != listening;
                self.listening = listening;
                if listening {
                    self.mic_error = None;
                }
                changed
            }
            ModelMessage::Behind(behind) => {
                let changed = self.behind != behind;
                self.behind = behind;
                changed
            }
        }
    }
}

/// Seconds as `m:ss` or `h:mm:ss`.
pub fn format_duration(seconds: u32) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

/// The state entity the window renders from.
pub struct SessionEntity {
    model: SessionModel,
    state: Arc<AppState>,
    /// The update drain.
    ///
    /// Held so it is cancelled when the entity drops. A detached task would
    /// outlive the window and keep pushing updates at a view nobody renders.
    ///
    /// An `Option` because `Task` has no no-op value: the field is empty for the
    /// moment between construction and the spawn that replaces it.
    _drain: Option<gpui::Task<()>>,
}

impl SessionEntity {
    /// Create the entity and start draining updates into it.
    pub fn new(
        state: Arc<AppState>,
        messages: async_channel::Receiver<ModelMessage>,
        overlay_url: String,
        cx: &mut gpui_kit::App,
    ) -> Entity<Self> {
        let model = SessionModel {
            overlay_theme: state.overlay_theme(None),
            overlay_url,
            ..SessionModel::default()
        };

        let entity = cx.new(|_cx| Self {
            model,
            state,
            _drain: None,
        });

        // `cx.spawn` on the entity, so the future is cancelled with it. The weak
        // handle it passes in is what makes that safe: the task does not keep the
        // entity alive, so closing the window drops the entity and the task ends.
        // The receiver is moved into the task rather than read off the entity:
        // `cx.spawn` passes a `WeakEntity`, so the future cannot touch the
        // entity's fields directly and only reaches it by upgrading.
        let receiver = messages.clone();

        entity.update(cx, |this, cx| {
            this._drain = Some(cx.spawn(async move |entity, cx| {
                // A closed channel means the background work has stopped, which
                // is the only way this loop ends. `recv` needs no reactor, which
                // is why this is an async channel rather than a tokio one: GPUI's
                // executor is not tokio's.
                while let Ok(message) = receiver.recv().await {
                    // A failed update means the entity is gone. Stop rather than
                    // spinning on a dead handle.
                    //
                    // The `notify` is the load-bearing part. This entity is what
                    // the view observes, so without it the model updates and the
                    // window never learns: the count advances in memory through
                    // an entire yapathon while the number on screen stays where
                    // it was. Nothing errors and nothing logs, because from the
                    // model's point of view everything worked.
                    if entity
                        .update(cx, |this, cx| {
                            if this.model.apply(message) {
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        });

        entity
    }

    /// The current model, for rendering.
    pub fn model(&self) -> &SessionModel {
        &self.model
    }

    /// The shared state, for issuing commands.
    pub fn state(&self) -> &Arc<AppState> {
        &self.state
    }

    /// Change the overlay theme and tell the server.
    ///
    /// Both at once, because they have to agree: the window offers a URL with
    /// the theme in it and the server serves whatever that URL asks for. Setting
    /// one without the other is how a streamer copies a `?theme=cute` URL and
    /// gets the default card.
    pub fn set_overlay_theme(&mut self, theme: &str, cx: &mut gpui::Context<Self>) {
        self.model.overlay_theme = self.state.overlay_theme(Some(theme));
        cx.notify();
    }

    /// Set the microphone error, which the controls render in place of
    /// themselves.
    pub fn set_mic_error(&mut self, message: Option<String>, cx: &mut gpui::Context<Self>) {
        self.model.mic_error = message;
        cx.notify();
    }

    /// Replace the model wholesale, for the initial load.
    ///
    /// Notifies, for the same reason the drain task does: this is the first
    /// state the window will ever see, and a view that subscribed before the
    /// load has nothing to draw without it.
    pub fn hydrate(
        &mut self,
        snapshot: SessionSnapshot,
        transcript: Vec<TranscriptLine>,
        cx: &mut gpui::Context<Self>,
    ) {
        self.model.snapshot = Some(snapshot);
        self.model.transcript = transcript;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yap_core::goal::HardLimit;
    use yap_core::session::SessionConfig;

    fn snapshot(mode: GoalMode) -> SessionSnapshot {
        let mut session = yap_core::session::Session::new(SessionConfig::new(mode, None));
        session.start();
        session.snapshot()
    }

    fn model_with(snapshot: SessionSnapshot) -> SessionModel {
        SessionModel {
            snapshot: Some(snapshot),
            ..SessionModel::default()
        }
    }

    #[test]
    fn an_empty_model_reads_as_zero_not_as_a_crash() {
        // The window opens before a yapathon starts.
        let model = SessionModel::default();
        assert_eq!(model.headline(), "0");
        assert_eq!(model.headline_unit(), "words");
        assert_eq!(model.remaining(), "—");
        assert_eq!(model.rate(), "— wpm");
        assert_eq!(model.status_label(), "no yapathon");
        assert_eq!(model.fraction(), 0.0);
        assert!(model.can_start());
    }

    #[test]
    fn a_word_run_shows_words_and_remaining() {
        let model = model_with(snapshot(GoalMode::Fixed { words: 5000 }));
        assert_eq!(model.headline(), "0");
        assert_eq!(model.headline_unit(), "words");
        assert_eq!(model.goal_line(), "of 5000 words");
        assert_eq!(model.remaining(), "5000 to go");
    }

    #[test]
    fn a_time_run_shows_a_clock() {
        let model = model_with(snapshot(GoalMode::Time {
            base: std::time::Duration::from_secs(600),
        }));

        // A clock, not a word total.
        assert_eq!(model.headline_unit(), "left");
        assert_eq!(model.headline(), "10:00");
        assert_eq!(model.goal_line(), "0:00 elapsed");
    }

    #[test]
    fn durations_format_the_way_a_streamer_reads_them() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(9), "0:09");
        assert_eq!(format_duration(75), "1:15");
        assert_eq!(format_duration(3600), "1:00:00");
        assert_eq!(format_duration(3725), "1:02:05");
    }

    #[test]
    fn button_state_follows_the_status() {
        let mut session = yap_core::session::Session::new(SessionConfig::new(
            GoalMode::Fixed { words: 100 },
            None,
        ));
        session.start();
        let mut model = model_with(session.snapshot());

        assert!(!model.can_start(), "cannot start twice");
        assert!(model.can_pause());
        assert!(!model.can_resume());

        session.pause();
        model.apply(ModelMessage::State(session.snapshot()));

        assert!(!model.can_pause());
        assert!(model.can_resume());
    }

    #[test]
    fn undo_needs_counted_words() {
        let mut session = yap_core::session::Session::new(SessionConfig::new(
            GoalMode::Fixed { words: 100 },
            None,
        ));
        session.start();
        let mut model = model_with(session.snapshot());

        assert!(!model.can_undo());
        session.record("one two three", 0, 1000);
        model.apply(ModelMessage::State(session.snapshot()));
        assert!(model.can_undo());
    }

    #[test]
    fn the_event_feed_is_bounded() {
        let mut model = SessionModel::default();
        for i in 0..300 {
            model.apply(ModelMessage::Event(Event::granting("sub", i)));
        }

        assert_eq!(model.events.len(), 200);
        // The newest survive, which is what a feed shows.
        assert_eq!(model.events.last().map(|e| e.words), Some(Some(299)));
    }

    #[test]
    fn warnings_are_bounded() {
        let mut model = SessionModel::default();
        for i in 0..80 {
            model.apply(ModelMessage::Warning(format!("warning {i}")));
        }
        assert_eq!(model.warnings.len(), 50);
    }

    #[test]
    fn starting_to_listen_clears_a_previous_error() {
        let mut model = SessionModel {
            mic_error: Some("device busy".to_string()),
            ..SessionModel::default()
        };

        model.apply(ModelMessage::Listening(true));
        assert_eq!(model.mic_error, None);
    }

    #[test]
    fn being_behind_is_visible() {
        // A counter that is behind the streamer's voice must say so rather than
        // presenting a stale number as current.
        let mut model = model_with(snapshot(GoalMode::Fixed { words: 100 }));
        assert!(!model.behind);

        model.apply(ModelMessage::Behind(true));
        assert!(model.behind);
    }

    #[test]
    fn a_capped_run_is_flagged() {
        let mut session = yap_core::session::Session::new(SessionConfig::new(
            GoalMode::Extend { base_words: 100 },
            Some(HardLimit::words(100)),
        ));
        session.start();
        session.apply_event(&Event::granting("sub", 100));

        let model = model_with(session.snapshot());
        assert!(model.is_capped());
    }

    #[test]
    fn chat_totals_add_score_and_bonus() {
        let mut session = yap_core::session::Session::new(SessionConfig::new(
            GoalMode::Extend { base_words: 100 },
            None,
        ));
        session.start();
        session.apply_event(&Event::granting("sub", 100));
        session.apply_event(&Event::granting("sub", 50));

        let model = model_with(session.snapshot());
        assert_eq!(model.from_chat(), "150");
    }
}
