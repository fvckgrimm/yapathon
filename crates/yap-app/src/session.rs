//! Turning UI intent into state changes.
//!
//! Every button on the dock goes through one of these. Each is a thin async
//! function on the shared [`AppState`], which means the same operations are
//! reachable from the HTTP API and from a chat command — there is one path to
//! the goal math, not three that have to agree.
//!
//! These deliberately do not touch GPUI. A command that awaits tokio cannot be
//! called from a render, so the UI spawns them and observes the result through
//! the same broadcast the overlay uses.

use std::sync::Arc;

use yap_core::goal::{GoalMode, HardLimit};
use yap_core::session::SessionConfig;
use yap_server::AppState;

/// What a new yapathon should be.
#[derive(Debug, Clone, PartialEq)]
pub struct StartRequest {
    pub mode: GoalMode,
    /// Hard limit in the mode's own unit: words, or seconds for a time run.
    pub hard_limit: Option<u32>,
}

impl StartRequest {
    /// An extend run: the goal, and how far chat can push it.
    ///
    /// A goal with no limit is reachable but unbounded, which is a legitimate
    /// choice and a deliberate one — so it stays possible rather than being
    /// papered over with a default the streamer did not ask for.
    pub fn extend(goal: u32, limit: Option<u32>) -> Self {
        Self {
            mode: GoalMode::Extend { base_words: goal },
            hard_limit: limit,
        }
    }
}

impl Default for StartRequest {
    fn default() -> Self {
        Self::extend(5_000, Some(10_000))
    }
}

/// Start a yapathon.
pub async fn start(state: &Arc<AppState>, request: StartRequest) -> anyhow::Result<()> {
    let limit = request.hard_limit.map(|max| HardLimit { max });
    state
        .start_session(SessionConfig::new(request.mode, limit))
        .await?;
    Ok(())
}

pub async fn pause(state: &Arc<AppState>) -> anyhow::Result<()> {
    state.pause().await?;
    Ok(())
}

pub async fn resume(state: &Arc<AppState>) -> anyhow::Result<()> {
    state.resume().await?;
    Ok(())
}

pub async fn end(state: &Arc<AppState>) -> anyhow::Result<()> {
    state.end().await?;
    Ok(())
}

/// Void the last utterance, reporting whether there was one.
///
/// The answer matters to the UI: an undo that silently did nothing looks like a
/// button that is broken.
pub async fn undo(state: &Arc<AppState>) -> anyhow::Result<bool> {
    Ok(state.undo_last().await?.is_some())
}

/// Apply a manual correction in words.
pub async fn adjust(state: &Arc<AppState>, words: i64, note: &str) -> anyhow::Result<()> {
    let note = if note.trim().is_empty() {
        "manual adjustment"
    } else {
        note
    };
    state.adjust(words, note).await?;
    Ok(())
}

/// Apply one platform or chat event.
pub async fn apply_event(state: &Arc<AppState>, event: yap_core::goal::Event) -> u32 {
    state.apply_event(event).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_request_is_an_extend_run_with_a_limit() {
        // Uncapped by default would be a trap: chat could extend a goal forever
        // and the yapathon would never end.
        let request = StartRequest::default();
        assert_eq!(request.mode, GoalMode::Extend { base_words: 5_000 });
        assert_eq!(request.hard_limit, Some(10_000));
    }

    #[tokio::test]
    async fn starting_and_pausing_a_run() {
        let state = AppState::shared();
        start(&state, StartRequest::default())
            .await
            .expect("started");

        assert!(state.status().await.is_some());

        pause(&state).await.expect("paused");
        assert!(!state.status().await.expect("status").is_active());

        resume(&state).await.expect("resumed");
        assert!(state.status().await.expect("status").is_active());

        end(&state).await.expect("ended");
    }

    #[tokio::test]
    async fn undo_reports_whether_it_did_anything() {
        let state = AppState::shared();
        start(&state, StartRequest::default())
            .await
            .expect("started");

        assert!(
            !undo(&state).await.expect("no session words"),
            "nothing to undo"
        );

        state.record_words("one two three", 0, 1000).await;
        assert!(undo(&state).await.expect("undone"));
    }

    #[tokio::test]
    async fn adjusting_with_an_empty_note_gets_a_default_one() {
        let state = AppState::shared();
        start(&state, StartRequest::default())
            .await
            .expect("started");
        state.record_words("one two", 0, 1000).await;

        adjust(&state, 10, "   ").await.expect("adjusted");
        assert_eq!(state.snapshot().await.expect("snapshot").words, 12);
    }

    #[tokio::test]
    async fn commands_without_a_session_report_an_error() {
        let state = AppState::shared();
        assert!(pause(&state).await.is_err());
        assert!(end(&state).await.is_err());
    }
}
