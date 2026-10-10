//! Yapathon goals: how many words end the run, and what moves the line.
//!
//! Three modes, chosen per yapathon rather than per install, because the mode
//! is what makes one of these runs interesting:
//!
//! - [`GoalMode::Fixed`] — the goal never moves. Events grant score only.
//! - [`GoalMode::Extend`] — the goal grows by the event bonus. The classic
//!   "chat keeps adding words" run, and the one that needs a hard limit.
//! - [`GoalMode::Time`] — the goal is a duration and events extend the clock.
//!
//! The hard limit is the reason [`GoalMode::Extend`] terminates at all. Without
//! it a busy chat makes the run unbounded; with it the run always has a real
//! ending, which is the thing a yapathon needs.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// What the run is trying to achieve, and whether chat can move it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GoalMode {
    /// A fixed number of words. Events add score, never goal.
    Fixed {
        /// Words required to finish.
        words: u32,
    },
    /// A word goal that grows with events.
    Extend {
        /// Words required before any bonus is applied.
        base_words: u32,
    },
    /// A clock. Events push the finish further away.
    Time {
        /// Base duration.
        base: Duration,
    },
}

impl GoalMode {
    /// The short name for this mode: `extend`, `fixed` or `time`.
    ///
    /// Not `Debug`, which renders `Extend { base_words: 5000 }`. A settings panel
    /// offering that as a choice value would match no button and show none
    /// selected, so the mode looked unsettable even though it was set.
    pub fn short_name(&self) -> &'static str {
        match self {
            Self::Fixed { .. } => "fixed",
            Self::Extend { .. } => "extend",
            Self::Time { .. } => "time",
        }
    }

    /// Parse a short name back into a mode with the given target.
    ///
    /// Returns `None` for an unrecognised name rather than defaulting, so a
    /// settings file naming a mode that does not exist is visible instead of
    /// silently becoming something else.
    pub fn from_short_name(name: &str, words: u32) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "fixed" => Some(Self::Fixed { words }),
            "extend" => Some(Self::Extend { base_words: words }),
            "time" => Some(Self::Time {
                base: std::time::Duration::from_secs(u64::from(words)),
            }),
            _ => None,
        }
    }

    /// Every mode's short name, for a settings panel to offer as choices.
    pub fn short_names() -> [&'static str; 3] {
        ["fixed", "extend", "time"]
    }

    /// The starting target with no bonus applied. For [`GoalMode::Time`] this
    /// is a duration, for the other two a word count.
    pub fn base_words(&self) -> Option<u32> {
        match self {
            Self::Fixed { words } => Some(*words),
            Self::Extend { base_words } => Some(*base_words),
            Self::Time { .. } => None,
        }
    }

    pub fn base_duration(&self) -> Option<Duration> {
        match self {
            Self::Time { base } => Some(*base),
            _ => None,
        }
    }

    /// A short label for the overlay.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Fixed { .. } => "fixed goal",
            Self::Extend { .. } => "chat extends goal",
            Self::Time { .. } => "chat extends clock",
        }
    }
}

/// The ceiling on a goal.
///
/// In [`GoalMode::Time`] this caps the total duration. In the word modes it
/// caps the goal as it grows. `None` means uncapped, which is a valid choice
/// for a fixed goal but makes an extend run unfinishable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HardLimit {
    /// Maximum goal in the mode's own unit: words, or seconds for
    /// [`GoalMode::Time`].
    pub max: u32,
}

impl HardLimit {
    pub fn words(max: u32) -> Self {
        Self { max }
    }

    pub fn duration(d: Duration) -> Self {
        Self {
            max: d.as_secs() as u32,
        }
    }
}

/// One thing that happened on a stream, normalised across platforms.
///
/// A Twitch resub, a YouTube super chat and a `!yap extend 500` chat command
/// all become the same shape, which is what keeps the platform trait from
/// leaking into the goal math.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Where it came from. `chat` for a typed command, otherwise the platform.
    pub platform: String,
    /// The event type, as the platform names it: `sub`, `resub`, `gift_sub`,
    /// `cheer`, `raid`, `redemption`, `donation`, `chat`.
    pub kind: String,
    /// Who caused it, when known.
    pub user: Option<String>,
    /// Platform-native amount: bits cheered, dollars donated, subscriber
    /// months. Present because the configured word value is often a multiple
    /// or a divisor of this.
    pub amount: Option<f64>,
    /// Words this event granted. `None` for kinds the streamer has not priced,
    /// which is different from granting zero.
    pub words: Option<u32>,
    /// Free-form note for the event feed, e.g. the reward's name.
    pub note: Option<String>,
}

impl Event {
    /// An event that grants a fixed number of words.
    pub fn granting(kind: &str, words: u32) -> Self {
        Self {
            platform: "chat".to_string(),
            kind: kind.to_string(),
            user: None,
            amount: None,
            words: Some(words),
            note: None,
        }
    }

    /// Words this event contributes, or zero if unpriced. Events without a
    /// price still reach the feed; they just do not move the goal.
    pub fn granted_words(&self) -> u32 {
        self.words.unwrap_or(0)
    }
}

/// The current state of a run's goal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoalProgress {
    /// Words required to finish, or seconds remaining for a time run.
    pub target: u32,
    /// Words counted, or seconds elapsed for a time run.
    pub achieved: u32,
    /// Whether the hard limit has been reached, meaning no further event can
    /// extend the goal.
    pub capped: bool,
}

impl GoalProgress {
    /// How far along the run is, in the range 0.0..=1.0.
    ///
    /// Returns 1.0 for a zero target rather than dividing by zero. A goal of
    /// zero words is finished, which is a legal if silly configuration.
    pub fn fraction(&self) -> f32 {
        if self.target == 0 {
            return 1.0;
        }
        (self.achieved as f32 / self.target as f32).clamp(0.0, 1.0)
    }

    /// Words or seconds still required. Zero once complete.
    pub fn remaining(&self) -> u32 {
        self.target.saturating_sub(self.achieved)
    }

    /// Has the goal been met?
    pub fn complete(&self) -> bool {
        self.achieved >= self.target
    }
}

/// Accumulates events against a goal, applying the mode and the hard limit.
///
/// This is a pure value type on purpose: it holds no clock, no I/O and no
/// handle to the session. Time-based progression is fed in as elapsed seconds,
/// so the whole thing is testable without sleeping.
#[derive(Debug, Clone)]
pub struct GoalTracker {
    mode: GoalMode,
    limit: Option<HardLimit>,
    /// Words granted by events that do not move the goal.
    score: u64,
    /// Total words granted by events, whether or not they moved the goal. Kept
    /// so the overlay can show both "score" and "bonus" without re-summing.
    bonus_total: u64,
    /// Word goal after extension. `None` in [`GoalMode::Time`].
    goal: Option<u32>,
    /// Total extension in seconds. `None` outside [`GoalMode::Time`].
    extended: Option<u32>,
    capped: bool,
}

impl GoalTracker {
    pub fn new(mode: GoalMode, limit: Option<HardLimit>) -> Self {
        let goal = mode.base_words();
        let extended = mode.base_duration().map(|d| d.as_secs() as u32);

        Self {
            mode,
            limit,
            score: 0,
            bonus_total: 0,
            goal,
            extended,
            capped: false,
        }
    }

    pub fn mode(&self) -> &GoalMode {
        &self.mode
    }

    pub fn limit(&self) -> Option<HardLimit> {
        self.limit
    }

    /// Score granted by events that did not move the goal. In
    /// [`GoalMode::Extend`] and [`GoalMode::Time`] this stays zero, because
    /// every granted word goes to the goal instead.
    pub fn score(&self) -> u64 {
        self.score
    }

    /// Words granted by events, whether they moved the goal or became score.
    pub fn bonus_total(&self) -> u64 {
        self.bonus_total
    }

    /// The current word goal, or the elapsed-time budget in seconds for a time
    /// run.
    pub fn target(&self) -> u32 {
        match (&self.goal, &self.extended) {
            (Some(words), _) => *words,
            (None, Some(seconds)) => *seconds,
            (None, None) => 0,
        }
    }

    /// The current target as a duration, for a time run.
    pub fn target_duration(&self) -> Option<Duration> {
        self.extended
            .map(|secs| Duration::from_secs(u64::from(secs)))
    }

    pub fn is_capped(&self) -> bool {
        self.capped
    }

    /// Apply one event.
    ///
    /// Returns the words the event actually contributed, which may be fewer
    /// than granted when the hard limit truncates the extension, or zero when
    /// the run is already capped or the kind is unpriced.
    pub fn apply(&mut self, event: &Event) -> u32 {
        let requested = event.granted_words();
        if requested == 0 {
            return 0;
        }

        // What this event actually contributed. The hard limit can truncate it,
        // and `bonus_total` has to record the truncated figure — otherwise a
        // capped run reports more bonus words than the goal ever moved by,
        // which is the kind of number that gets argued about in chat.
        let granted = match self.mode {
            // A fixed goal does not move. The event is worth score instead, and
            // it is not capped: score is not the finish line, so the hard limit
            // has nothing to truncate.
            GoalMode::Fixed { .. } => {
                self.score += u64::from(requested);
                requested
            }
            GoalMode::Extend { .. } => self.extend_goal(requested),
            GoalMode::Time { .. } => self.extend_clock(requested),
        };

        self.bonus_total += u64::from(granted);
        granted
    }

    /// Grow the word goal by `requested`, respecting the hard limit.
    ///
    /// Returns the words actually added, which is fewer than `requested` when
    /// the limit truncates it, and zero once the limit is already reached.
    fn extend_goal(&mut self, requested: u32) -> u32 {
        let current = self.goal.unwrap_or(0);

        match self.limit {
            Some(HardLimit { max }) if current.saturating_add(requested) > max => {
                let allowed = max.saturating_sub(current);
                self.goal = Some(max);
                self.capped = true;
                allowed
            }
            _ => {
                self.goal = Some(current.saturating_add(requested));
                requested
            }
        }
    }

    /// Extend the clock by `requested` seconds, respecting the hard limit.
    ///
    /// The same truncation as [`Self::extend_goal`], applied to a duration.
    fn extend_clock(&mut self, requested: u32) -> u32 {
        let current = self.extended.unwrap_or(0);

        match self.limit {
            Some(HardLimit { max }) if current.saturating_add(requested) > max => {
                let allowed = max.saturating_sub(current);
                self.extended = Some(max);
                self.capped = true;
                allowed
            }
            _ => {
                self.extended = Some(current.saturating_add(requested));
                requested
            }
        }
    }

    /// Progress against words counted, or against elapsed time in seconds.
    pub fn progress(&self, achieved: u32) -> GoalProgress {
        GoalProgress {
            target: self.target(),
            achieved,
            capped: self.capped,
        }
    }

    /// Estimated seconds to finish at a given speaking rate in words per
    /// minute. `None` when the rate is unknown or nothing remains, since a
    /// number computed from a zero rate is a lie.
    pub fn estimated_seconds_remaining(
        &self,
        achieved: u32,
        words_per_minute: f32,
    ) -> Option<Duration> {
        let remaining = self.progress(achieved).remaining();
        if remaining == 0 || words_per_minute <= 0.0 {
            return None;
        }
        let seconds = (f64::from(remaining) / f64::from(words_per_minute)) * 60.0;
        Some(Duration::from_secs_f64(seconds))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_goal_ignores_events() {
        let mut t = GoalTracker::new(GoalMode::Fixed { words: 1000 }, None);
        let applied = t.apply(&Event::granting("sub", 500));

        // The event is worth score but does not move the line.
        assert_eq!(applied, 500);
        assert_eq!(t.target(), 1000);
        assert_eq!(t.score(), 500);
        assert_eq!(t.bonus_total(), 500);
    }

    #[test]
    fn extend_goal_grows_by_bonus() {
        let mut t = GoalTracker::new(GoalMode::Extend { base_words: 1000 }, None);
        t.apply(&Event::granting("sub", 250));
        t.apply(&Event::granting("cheer", 100));

        assert_eq!(t.target(), 1350);
        assert_eq!(t.score(), 0, "extended words are goal, not score");
    }

    #[test]
    fn extend_goal_respects_hard_limit() {
        let mut t = GoalTracker::new(
            GoalMode::Extend { base_words: 1000 },
            Some(HardLimit::words(1500)),
        );

        assert_eq!(t.apply(&Event::granting("sub", 300)), 300);
        assert_eq!(t.target(), 1300);

        // The second event overshoots: only the room left is granted.
        assert_eq!(t.apply(&Event::granting("sub", 500)), 200);
        assert_eq!(t.target(), 1500);
        assert!(t.is_capped());

        // Once capped, events grant nothing further.
        assert_eq!(t.apply(&Event::granting("sub", 1000)), 0);
        assert_eq!(t.target(), 1500);
    }

    #[test]
    fn bonus_total_counts_what_was_granted_not_what_was_asked_for() {
        // The bug this guards: a capped run reporting more bonus words than the
        // goal ever moved by. 300 + 200 + 0, not 300 + 500 + 1000.
        let mut t = GoalTracker::new(
            GoalMode::Extend { base_words: 1000 },
            Some(HardLimit::words(1500)),
        );

        t.apply(&Event::granting("sub", 300));
        t.apply(&Event::granting("sub", 500));
        t.apply(&Event::granting("sub", 1000));

        assert_eq!(t.bonus_total(), 500);

        // And it agrees with how far the goal actually moved.
        assert_eq!(t.target() - 1000, 500);
    }

    #[test]
    fn a_capped_time_run_reports_only_the_extension_it_got() {
        let mut t = GoalTracker::new(
            GoalMode::Time {
                base: Duration::from_secs(600),
            },
            Some(HardLimit::duration(Duration::from_secs(900))),
        );

        assert_eq!(t.apply(&Event::granting("sub", 120)), 120);
        assert_eq!(t.apply(&Event::granting("raid", 500)), 180);
        assert_eq!(t.apply(&Event::granting("raid", 1000)), 0);

        assert_eq!(t.bonus_total(), 300);
        assert_eq!(t.target() - 600, 300);
    }

    #[test]
    fn a_fixed_goal_scores_everything_because_score_is_not_the_finish_line() {
        // Score is not capped by the hard limit: the limit governs the goal, and
        // in a fixed run there is no goal to move.
        let mut t = GoalTracker::new(
            GoalMode::Fixed { words: 1000 },
            Some(HardLimit::words(1500)),
        );

        assert_eq!(t.apply(&Event::granting("sub", 5000)), 5000);
        assert_eq!(t.score(), 5000);
        assert_eq!(t.bonus_total(), 5000);
        assert_eq!(t.target(), 1000);
        assert!(!t.is_capped());
    }

    #[test]
    fn unpriced_events_do_not_move_the_goal_but_are_logged() {
        let mut t = GoalTracker::new(GoalMode::Extend { base_words: 500 }, None);
        let event = Event {
            platform: "twitch".into(),
            kind: "follow".into(),
            user: Some("someone".into()),
            amount: None,
            words: None,
            note: None,
        };

        assert_eq!(t.apply(&event), 0);
        assert_eq!(t.target(), 500);
        assert_eq!(t.bonus_total(), 0);
    }

    #[test]
    fn time_mode_extends_the_clock() {
        let mut t = GoalTracker::new(
            GoalMode::Time {
                base: Duration::from_secs(600),
            },
            Some(HardLimit::duration(Duration::from_secs(900))),
        );

        assert_eq!(t.target(), 600);
        assert_eq!(t.target_duration(), Some(Duration::from_secs(600)));

        // A sub worth 120 words extends by 120 seconds.
        assert_eq!(t.apply(&Event::granting("sub", 120)), 120);
        assert_eq!(t.target(), 720);

        // A big event truncates at the limit rather than overshooting.
        assert_eq!(t.apply(&Event::granting("raid", 500)), 180);
        assert_eq!(t.target(), 900);
        assert!(t.is_capped());
    }

    #[test]
    fn extend_mode_without_a_limit_is_uncapped() {
        let mut t = GoalTracker::new(GoalMode::Extend { base_words: 100 }, None);
        for _ in 0..100 {
            t.apply(&Event::granting("sub", 100));
        }
        assert_eq!(t.target(), 10_100);
        assert!(!t.is_capped());
    }

    #[test]
    fn progress_reports_completion_and_fraction() {
        let t = GoalTracker::new(GoalMode::Fixed { words: 200 }, None);

        let p = t.progress(50);
        assert_eq!(p.remaining(), 150);
        assert!((p.fraction() - 0.25).abs() < f32::EPSILON);
        assert!(!p.complete());

        assert!(t.progress(200).complete());
        assert!(t.progress(250).complete());
    }

    #[test]
    fn zero_goal_is_finished_not_a_division_by_zero() {
        let t = GoalTracker::new(GoalMode::Fixed { words: 0 }, None);
        let p = t.progress(0);
        assert_eq!(p.fraction(), 1.0);
        assert_eq!(p.remaining(), 0);
        assert!(p.complete());
    }

    #[test]
    fn estimate_is_absent_without_a_rate() {
        let t = GoalTracker::new(GoalMode::Fixed { words: 1000 }, None);
        assert_eq!(t.estimated_seconds_remaining(0, 0.0), None);

        let est = t.estimated_seconds_remaining(0, 150.0).expect("rate given");
        assert_eq!(est, Duration::from_secs(400));
    }

    #[test]
    fn estimate_is_absent_once_complete() {
        let t = GoalTracker::new(GoalMode::Fixed { words: 100 }, None);
        assert_eq!(t.estimated_seconds_remaining(100, 150.0), None);
    }
}
