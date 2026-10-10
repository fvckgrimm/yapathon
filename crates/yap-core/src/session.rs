//! The session: what has been said, what chat has added, and when it ended.
//!
//! The audit log is the reason this type is more than a counter. A yapathon
//! is a public claim about how many words someone said, and chat will check it.
//! Every utterance's text and timestamps are kept so a disputed count can be
//! recomputed and shown, and so a recognition error can be undone rather than
//! argued about.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::counting::{CountingRules, count};
use crate::goal::{Event, GoalMode, GoalProgress, GoalTracker, HardLimit};

/// One recognised utterance, exactly as scored.
///
/// `words` is stored rather than recomputed on read so that the recorded count
/// is a fact about what happened, not a function of the current rules. Changing
/// the rules afterwards produces a new, separately-labelled total.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Utterance {
    /// When the utterance started, relative to session start.
    pub started_ms: u64,
    /// When it ended.
    pub ended_ms: u64,
    /// Raw recogniser output, kept verbatim.
    pub text: String,
    /// Scored words.
    pub words: u32,
    /// Filler words in this utterance.
    pub fillers: u32,
    /// Every token after normalisation, so the score can be explained.
    pub tokens: Vec<String>,
    /// Set when a human corrected or voided this entry. Voided utterances stay
    /// in the log and are excluded from totals; deleting them would destroy the
    /// evidence for the correction.
    #[serde(default)]
    pub voided: bool,
    /// Note explaining a void or correction.
    #[serde(default)]
    pub note: Option<String>,
}

impl Utterance {
    pub fn duration(&self) -> Duration {
        Duration::from_millis(self.ended_ms.saturating_sub(self.started_ms))
    }
}

/// Where a run is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Configured but not started.
    Idle,
    Running,
    /// Stopped by the streamer: AFK, technical difficulties, a game bug.
    /// Time keeps accumulating, but nothing is counted.
    Paused,
    /// Goal reached.
    Won,
    /// Hard limit reached without reaching the goal. The run is over by design,
    /// not by failure — this is the state that makes an extend run terminate.
    Capped,
    /// Ended early by choice.
    Ended,
}

impl SessionStatus {
    /// Whether the counter should still be moving.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Running)
    }

    /// Whether this is a real ending, as opposed to a pause or a draft.
    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Won | Self::Capped | Self::Ended)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "yapping",
            Self::Paused => "paused",
            Self::Won => "goal reached",
            Self::Capped => "hit the limit",
            Self::Ended => "ended",
        }
    }
}

/// Settings captured when a yapathon starts.
///
/// Frozen into the session so that changing a preference mid-run cannot
/// retroactively alter what the run meant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub mode: GoalMode,
    pub hard_limit: Option<HardLimit>,
    pub rules: CountingRules,
}

impl SessionConfig {
    pub fn new(mode: GoalMode, hard_limit: Option<HardLimit>) -> Self {
        Self {
            mode,
            hard_limit,
            rules: CountingRules::default(),
        }
    }

    pub fn with_rules(mut self, rules: CountingRules) -> Self {
        self.rules = rules;
        self
    }
}

/// Counted words over a run, broken out by reason.
///
/// The split exists because "how many words" is ambiguous in chat. Someone
/// asking will mean the goal-facing total; someone auditing wants to know how
/// much of it came from recognition rather than speech.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    /// Words actually spoken, as scored. This is what the goal is measured
    /// against.
    pub spoken: u32,
    /// Filler words inside `spoken`, not removed unless the rules say so.
    pub fillers: u32,
    /// Utterances voided by a correction.
    pub voided_utterances: u32,
    /// Utterances counted.
    pub utterances: u32,
}

impl Totals {
    /// Words with filler removed, regardless of the rules. This is the number a
    /// "no ums" variant compares against.
    pub fn substantive(&self) -> u32 {
        self.spoken.saturating_sub(self.fillers)
    }
}

/// A running or completed yapathon.
///
/// Holds a [`GoalTracker`] and the audit log. Deliberately free of I/O and of
/// wall-clock reads: elapsed time is reported by [`Session::elapsed`] against an
/// `Instant`, which the caller supplies, so the whole state machine is testable
/// without sleeping.
#[derive(Debug, Clone)]
pub struct Session {
    id: Uuid,
    config: SessionConfig,
    tracker: GoalTracker,
    status: SessionStatus,
    utterances: Vec<Utterance>,
    totals: Totals,
    /// Score from events in a fixed-goal run, kept for the summary. Derived
    /// from the tracker; stored so a finished session is self-contained.
    score: u64,
    started: Instant,
    /// Time spent paused, so elapsed speaking time can be reported.
    paused_total: Duration,
    /// When the current pause began, `None` while running.
    paused_at: Option<Instant>,
}

impl Session {
    /// Create a session. It starts [`SessionStatus::Idle`]; call
    /// [`Session::start`] to begin counting.
    pub fn new(config: SessionConfig) -> Self {
        let tracker = GoalTracker::new(config.mode.clone(), config.hard_limit);
        Self {
            id: Uuid::new_v4(),
            config,
            tracker,
            status: SessionStatus::Idle,
            utterances: Vec::new(),
            totals: Totals::default(),
            score: 0,
            started: Instant::now(),
            paused_total: Duration::ZERO,
            paused_at: None,
        }
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    pub fn status(&self) -> SessionStatus {
        self.status
    }

    pub fn tracker(&self) -> &GoalTracker {
        &self.tracker
    }

    pub fn totals(&self) -> Totals {
        self.totals
    }

    pub fn score(&self) -> u64 {
        self.score
    }

    /// Every utterance, voided included. The audit trail is not filtered.
    pub fn utterances(&self) -> &[Utterance] {
        &self.utterances
    }

    /// Begin counting. Starting a finished session is a no-op, so a
    /// double-clicked start button cannot reopen a completed run.
    pub fn start(&mut self) {
        if self.status.is_finished() {
            return;
        }
        self.status = SessionStatus::Running;
        self.started = Instant::now();
        self.paused_at = None;
    }

    /// Stop counting without ending the run. Paused time is excluded from
    /// elapsed, so a two-hour BRB does not look like two hours of silence
    /// between words.
    pub fn pause(&mut self) {
        if self.status == SessionStatus::Running {
            self.status = SessionStatus::Paused;
            self.paused_at = Some(Instant::now());
        }
    }

    /// Resume a paused run.
    pub fn resume(&mut self) {
        if self.status == SessionStatus::Paused {
            if let Some(at) = self.paused_at.take() {
                self.paused_total += at.elapsed();
            }
            self.status = SessionStatus::Running;
        }
    }

    /// End the run early.
    pub fn end(&mut self) {
        if !self.status.is_finished() {
            self.close_pause();
            self.status = SessionStatus::Ended;
        }
    }

    /// Fold the in-flight pause into `paused_total`. Idempotent.
    fn close_pause(&mut self) {
        if let Some(at) = self.paused_at.take() {
            self.paused_total += at.elapsed();
        }
    }

    /// Words counted so far, the number the goal is measured against.
    pub fn words(&self) -> u32 {
        self.totals.spoken
    }

    /// Words to go, or seconds of clock remaining.
    pub fn progress(&self) -> GoalProgress {
        let achieved = match self.config.mode {
            // A time run measures elapsed seconds, not words.
            GoalMode::Time { .. } => self.elapsed().as_secs() as u32,
            _ => self.totals.spoken,
        };
        self.tracker.progress(achieved)
    }

    /// Time since the run started, excluding pauses.
    pub fn elapsed(&self) -> Duration {
        let in_flight = match self.paused_at {
            Some(at) if self.status == SessionStatus::Paused => at.elapsed(),
            _ => Duration::ZERO,
        };
        self.started
            .elapsed()
            .saturating_sub(self.paused_total + in_flight)
    }

    /// Words per minute over speaking time. `None` before anything has been
    /// said, rather than zero, because a rate of zero is not a measurement.
    pub fn words_per_minute(&self) -> Option<f32> {
        let seconds = self.elapsed().as_secs_f32();
        if seconds <= 0.0 || self.totals.spoken == 0 {
            return None;
        }
        Some(f64::from(self.totals.spoken) as f32 / (seconds / 60.0))
    }

    /// Estimate when the run finishes at the current rate.
    pub fn estimated_remaining(&self) -> Option<Duration> {
        self.tracker
            .estimated_seconds_remaining(self.progress().achieved, self.words_per_minute()?)
    }

    /// Count one utterance and record it.
    ///
    /// Returns what was counted. Inert unless the session is running, so a
    /// recogniser that is still draining its buffer after a pause cannot leak
    /// words into a paused run.
    pub fn record(&mut self, text: &str, started_ms: u64, ended_ms: u64) -> u32 {
        if !self.status.is_active() {
            return 0;
        }

        let scored = count(text, &self.config.rules);
        self.totals.spoken = self.totals.spoken.saturating_add(scored.words);
        self.totals.fillers = self.totals.fillers.saturating_add(scored.fillers);
        self.totals.utterances += 1;

        self.utterances.push(Utterance {
            started_ms,
            ended_ms,
            text: text.to_string(),
            words: scored.words,
            fillers: scored.fillers,
            tokens: scored.tokens,
            voided: false,
            note: None,
        });

        // Reaching the goal ends the run. Capped is decided in `apply_event`,
        // which is the only place the goal can move.
        if self.progress().complete() && self.status.is_active() {
            self.status = SessionStatus::Won;
        }

        scored.words
    }

    /// Apply a platform or chat event.
    ///
    /// Returns the words actually granted, which the hard limit may have
    /// truncated. Moving the goal past the limit caps the run.
    pub fn apply_event(&mut self, event: &Event) -> u32 {
        let granted = self.tracker.apply(event);
        self.score = self.tracker.score();

        // A capped extend run is over. Without this check the run would sit at
        // a goal nothing can reach, waiting for a word count that may never
        // arrive.
        if self.tracker.is_capped() && self.status.is_active() {
            self.status = SessionStatus::Capped;
        }

        granted
    }

    /// Void the most recent live utterance, subtracting its words.
    ///
    /// This is the undo for a recognition error: a passing siren transcribed as
    /// a sentence. The entry stays in the log as voided, so the subtraction is
    /// auditable rather than invisible.
    ///
    /// Returns the words removed, or `None` if there is nothing to void.
    pub fn undo_last(&mut self) -> Option<u32> {
        let index = self.utterances.iter().rposition(|u| !u.voided)?;

        let entry = &self.utterances[index];
        let removed = entry.words;
        let fillers = entry.fillers;

        self.utterances[index].voided = true;
        self.utterances[index].note = Some("voided by streamer".to_string());

        self.totals.spoken = self.totals.spoken.saturating_sub(removed);
        self.totals.fillers = self.totals.fillers.saturating_sub(fillers);
        self.totals.utterances = self.totals.utterances.saturating_sub(1);
        self.totals.voided_utterances += 1;

        Some(removed)
    }

    /// Void a specific utterance by index into [`Session::utterances`].
    ///
    /// Returns the words removed. Already-voided entries are rejected rather
    /// than double-subtracted.
    pub fn void_at(&mut self, index: usize) -> Option<u32> {
        let entry = self.utterances.get(index)?;
        if entry.voided {
            return None;
        }

        let removed = entry.words;
        let fillers = entry.fillers;
        self.utterances[index].voided = true;

        self.totals.spoken = self.totals.spoken.saturating_sub(removed);
        self.totals.fillers = self.totals.fillers.saturating_sub(fillers);
        self.totals.utterances = self.totals.utterances.saturating_sub(1);
        self.totals.voided_utterances += 1;

        Some(removed)
    }

    /// Set a manual correction, in words, against the run.
    ///
    /// For the case an undo cannot cover: a whole misheard stretch the streamer
    /// wants to discount, or a genuine stream where speech recognition missed
    /// something they know they said.
    pub fn adjust(&mut self, words: i64, note: &str) -> u32 {
        log::info!("manual adjustment of {words} words: {note}");
        if words >= 0 {
            let add = u32::try_from(words).unwrap_or(0);
            self.totals.spoken = self.totals.spoken.saturating_add(add);
            return add;
        }

        // Subtracting twice would take a correction of 20 off a total of 52 and
        // leave 12. One subtraction, clamped at zero.
        let sub = u32::try_from(words.unsigned_abs()).unwrap_or(u32::MAX);
        let removed = self.totals.spoken.min(sub);
        self.totals.spoken -= removed;
        removed
    }

    /// Recount the whole session under different rules, without disturbing the
    /// recorded counts.
    ///
    /// This is the "prove it" path: the streamer applies the strictest
    /// interpretation and shows chat what the run is worth under it.
    pub fn recount(&self, rules: &CountingRules) -> Totals {
        let mut totals = Totals::default();

        for utterance in &self.utterances {
            if utterance.voided {
                continue;
            }
            let scored = count(&utterance.text, rules);
            totals.spoken = totals.spoken.saturating_add(scored.words);
            totals.fillers = totals.fillers.saturating_add(scored.fillers);
            totals.utterances += 1;
        }

        totals
    }

    /// A snapshot suitable for the overlay and the REST API.
    pub fn snapshot(&self) -> SessionSnapshot {
        let progress = self.progress();
        let remaining = progress.remaining();
        let estimated = self.estimated_remaining();

        SessionSnapshot {
            id: self.id,
            status: self.status,
            status_label: self.status.label().to_string(),
            mode: self.config.mode.clone(),
            mode_label: self.config.mode.label().to_string(),
            words: self.totals.spoken,
            fillers: self.totals.fillers,
            utterances: self.totals.utterances,
            target: progress.target,
            achieved: progress.achieved,
            remaining,
            fraction: progress.fraction(),
            complete: progress.complete(),
            capped: self.tracker.is_capped(),
            score: self.score,
            bonus_total: self.tracker.bonus_total(),
            words_per_minute: self.words_per_minute(),
            elapsed_secs: self.elapsed().as_secs(),
            estimated_remaining_secs: estimated.map(|d| d.as_secs()),
            recent: self.recent_utterances(5),
        }
    }

    /// The last `n` live utterances, newest last.
    pub fn recent_utterances(&self, n: usize) -> Vec<RecentUtterance> {
        // `filter` is not `DoubleEnded`, so collect backwards then reverse.
        // The log is short enough that this is not worth a fancier iterator,
        // and the clarity is worth more than the allocation.
        let mut out: Vec<RecentUtterance> = self
            .utterances
            .iter()
            .rev()
            .filter(|u| !u.voided)
            .take(n)
            .map(|u| RecentUtterance {
                text: u.text.clone(),
                words: u.words,
                at_ms: u.ended_ms,
            })
            .collect();

        out.reverse();
        out
    }
}

/// A serialisable view of a session, for the API and the overlay.
///
/// `PartialEq` is what lets a UI ask "did this actually change?" instead of
/// guessing. A running session publishes a snapshot several times a second, most
/// of them identical; without this, every consumer either repaints constantly or
/// never repaints at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: Uuid,
    pub status: SessionStatus,
    pub status_label: String,
    pub mode: GoalMode,
    pub mode_label: String,
    pub words: u32,
    pub fillers: u32,
    pub utterances: u32,
    pub target: u32,
    /// Words counted, or seconds elapsed for a time run. Paired with `target`
    /// so a clock can be rendered from the same two numbers as a word count.
    pub achieved: u32,
    pub remaining: u32,
    pub fraction: f32,
    pub complete: bool,
    pub capped: bool,
    pub score: u64,
    pub bonus_total: u64,
    pub words_per_minute: Option<f32>,
    pub elapsed_secs: u64,
    pub estimated_remaining_secs: Option<u64>,
    pub recent: Vec<RecentUtterance>,
}

/// One line of the transcript tail.
///
/// `PartialEq` so a `SessionSnapshot` containing it can be compared; see
/// [`SessionSnapshot`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentUtterance {
    pub text: String,
    pub words: u32,
    pub at_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::HardLimit;

    fn words_session(target: u32) -> Session {
        let config = SessionConfig::new(GoalMode::Fixed { words: target }, None);
        let mut s = Session::new(config);
        s.start();
        s
    }

    #[test]
    fn idle_session_counts_nothing() {
        // Deliberately not started: a session is inert until `start`.
        let mut s = Session::new(SessionConfig::new(GoalMode::Fixed { words: 1000 }, None));
        assert_eq!(s.status(), SessionStatus::Idle);
        assert_eq!(s.record("hello there", 0, 100), 0);
        assert_eq!(s.words(), 0);
    }

    #[test]
    fn records_and_totals_utterances() {
        let mut s = words_session(1000);
        s.record("one two three", 0, 1000);
        s.record("four five", 1000, 1500);

        assert_eq!(s.words(), 5);
        assert_eq!(s.totals().utterances, 2);
        assert_eq!(s.utterances().len(), 2);
    }

    #[test]
    fn paused_session_ignores_recogniser_drain() {
        let mut s = words_session(1000);
        s.record("one two", 0, 1000);
        s.pause();

        // The recogniser finishing a buffered utterance after the pause must
        // not leak into the run.
        assert_eq!(s.record("three four", 1000, 2000), 0);
        assert_eq!(s.words(), 2);
        assert_eq!(s.status(), SessionStatus::Paused);
    }

    #[test]
    fn reaching_the_goal_wins() {
        let mut s = words_session(5);
        s.record("one two three", 0, 1000);
        assert_eq!(s.status(), SessionStatus::Running);

        s.record("four five", 1000, 2000);
        assert_eq!(s.status(), SessionStatus::Won);
        assert!(s.progress().complete());
    }

    #[test]
    fn extend_run_caps_itself() {
        let config = SessionConfig::new(
            GoalMode::Extend { base_words: 100 },
            Some(HardLimit::words(150)),
        );
        let mut s = Session::new(config);
        s.start();

        s.apply_event(&Event::granting("sub", 25));
        assert_eq!(s.status(), SessionStatus::Running);
        assert_eq!(s.progress().target, 125);

        // This event takes the goal to 175, past the 150 limit.
        let granted = s.apply_event(&Event::granting("sub", 50));
        assert_eq!(granted, 25);
        assert_eq!(s.progress().target, 150);
        assert_eq!(s.status(), SessionStatus::Capped);
    }

    #[test]
    fn undo_removes_words_but_keeps_the_evidence() {
        let mut s = words_session(1000);
        s.record("one two three", 0, 1000);
        s.record("four five", 1000, 2000);
        assert_eq!(s.words(), 5);

        assert_eq!(s.undo_last(), Some(2));
        assert_eq!(s.words(), 3);

        // The entry is still in the log, marked voided.
        assert_eq!(s.utterances().len(), 2);
        assert!(s.utterances()[1].voided);
        assert_eq!(s.totals().voided_utterances, 1);

        // The filler tally is corrected by the same amount.
        assert_eq!(s.totals().fillers, 0);
    }

    #[test]
    fn undo_with_nothing_live_returns_none() {
        let mut s = words_session(1000);
        assert_eq!(s.undo_last(), None);
    }

    #[test]
    fn undo_twice_does_not_double_subtract() {
        let mut s = words_session(1000);
        s.record("one two three", 0, 1000);
        assert_eq!(s.undo_last(), Some(3));
        assert_eq!(s.undo_last(), None);
        assert_eq!(s.words(), 0);
    }

    #[test]
    fn void_by_index_rejects_an_already_voided_entry() {
        let mut s = words_session(1000);
        s.record("one two", 0, 1000);
        s.record("three four", 1000, 2000);

        assert_eq!(s.void_at(0), Some(2));
        assert_eq!(s.words(), 2);
        assert_eq!(s.void_at(0), None);
        assert_eq!(s.words(), 2);
    }

    #[test]
    fn recount_uses_the_original_text() {
        let mut config = SessionConfig::new(GoalMode::Fixed { words: 1000 }, None);
        config.rules.exclude_fillers = false;
        let mut s = Session::new(config);
        s.start();

        s.record("um so this is three four", 0, 1000);
        assert_eq!(s.words(), 6);

        // The same text, scored without fillers.
        let strict = CountingRules::default().excluding_fillers();
        let recounted = s.recount(&strict);
        assert_eq!(recounted.spoken, 5);
        assert_eq!(recounted.fillers, 1);

        // Recounting does not disturb the recorded totals.
        assert_eq!(s.words(), 6);
    }

    #[test]
    fn started_session_cannot_be_reopened_after_finishing() {
        let mut s = words_session(2);
        s.record("one two", 0, 1000);
        assert_eq!(s.status(), SessionStatus::Won);

        s.start();
        assert_eq!(s.status(), SessionStatus::Won);
    }

    #[test]
    fn end_stops_an_active_run() {
        let mut s = words_session(1000);
        s.record("one two", 0, 1000);
        s.end();

        assert_eq!(s.status(), SessionStatus::Ended);
        assert_eq!(s.record("three four", 1000, 2000), 0);
        assert_eq!(s.words(), 2);
    }

    #[test]
    fn snapshot_reports_the_essentials() {
        let mut s = words_session(100);
        s.record("one two three four", 0, 2000);

        let snap = s.snapshot();
        assert_eq!(snap.words, 4);
        assert_eq!(snap.target, 100);
        assert_eq!(snap.remaining, 96);
        assert_eq!(snap.status, SessionStatus::Running);
        assert!(!snap.complete);
        assert_eq!(snap.recent.len(), 1);
        assert!((snap.fraction - 0.04).abs() < 1e-6);
    }

    #[test]
    fn recent_utterances_excludes_voided_entries() {
        let mut s = words_session(1000);
        s.record("one two three", 0, 1000);
        s.record("four five", 1000, 2000);
        s.undo_last();

        let recent = s.recent_utterances(5);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].words, 3);
    }

    #[test]
    fn rate_is_absent_before_any_words() {
        let s = words_session(1000);
        assert_eq!(s.words_per_minute(), None);
        assert_eq!(s.estimated_remaining(), None);
    }

    #[test]
    fn time_mode_measures_the_clock_not_words() {
        let config = SessionConfig::new(
            GoalMode::Time {
                base: Duration::from_secs(60),
            },
            None,
        );
        let mut s = Session::new(config);
        s.start();
        s.record("a few words here", 0, 1000);

        // Achieved tracks elapsed seconds, so words do not move the bar.
        assert_eq!(s.progress().achieved, 0);
        assert_eq!(s.progress().target, 60);
    }
}
