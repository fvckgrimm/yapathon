//! A yapathon, end to end.
//!
//! This crate is everything that decides whether a yapathon is fair: how words
//! are counted, how the goal moves, and what the record is afterwards. It has
//! no audio capture, no network and no window, so all of it is testable by
//! feeding it text.
//!
//! The design constraint that shaped the rest: **a word is counted only from a
//! complete utterance.** See [`engine`] for why that matters.

pub mod counting;
pub mod engine;
pub mod goal;
pub mod session;
pub mod settings;

pub use counting::{Count, CountingRules, count, number_to_words};
pub use engine::{EngineError, SAMPLE_RATE, Transcript, TranscriptionEngine, Utterance, VadConfig};
pub use goal::{Event, GoalMode, GoalProgress, GoalTracker, HardLimit};
pub use session::{
    RecentUtterance, Session, SessionConfig, SessionSnapshot, SessionStatus, Totals,
    Utterance as LoggedUtterance,
};
pub use settings::{EngineChoice, Settings};
