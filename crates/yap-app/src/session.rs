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

/// A one-tap fake event for Test Mode.
///
/// StreamElements' simulator is the real way to test, but it is a whole second
/// app to drive. These fire the same priced event a simulator frame would have
/// produced, without leaving the window.
#[derive(Debug, Clone, Copy)]
pub struct FakePreset {
    /// What the button says, e.g. "Fake sub".
    pub label: &'static str,
    /// Priced under this platform, e.g. "twitch".
    pub platform: &'static str,
    /// Priced as this kind, e.g. "sub".
    pub kind: &'static str,
    /// The settings field pricing it, e.g. "event_twitch_sub".
    pub field: &'static str,
    /// How big the fake is, e.g. 100 bits. `None` means flat — one sub is one
    /// sub, and the configured value is the whole award.
    pub amount: Option<f64>,
}

/// The fakes on offer. One per common case, not one per priceable kind: five
/// buttons answer "does chat move the goal", twenty would ask "which one".
pub const FAKE_PRESETS: &[FakePreset] = &[
    FakePreset {
        label: "Fake sub",
        platform: "twitch",
        kind: "sub",
        field: "event_twitch_sub",
        amount: None,
    },
    FakePreset {
        label: "Fake cheer",
        platform: "twitch",
        kind: "cheer",
        field: "event_twitch_cheer",
        amount: Some(100.0),
    },
    FakePreset {
        label: "Fake raid",
        platform: "twitch",
        kind: "raid",
        field: "event_twitch_raid",
        amount: Some(10.0),
    },
    FakePreset {
        label: "Fake Super Chat",
        platform: "youtube",
        kind: "superchat",
        field: "event_youtube_superchat",
        amount: Some(5.0),
    },
    FakePreset {
        label: "Fake !yap",
        platform: "chat",
        kind: "command",
        field: "event_chat_command",
        amount: None,
    },
];

/// Build the fake event, priced from the settings panel's live value.
///
/// Reads the panel rather than a snapshot, so a fake agrees with what is on
/// screen even after Save — the connections snapshot pricing at startup, but a
/// test that disagrees with the visible price is a test that lies. A blank or
/// junk value prices nothing, the same rule `apply_field` uses: an unpriced
/// fake lands in the feed as "off" rather than moving the goal.
///
/// Labelled `(mock)` exactly like a simulator event, for the same reason: a
/// test session indistinguishable from a real one is worse than no test mode.
pub fn fake_event(preset: &FakePreset, panel_value: &str) -> yap_core::goal::Event {
    let configured = panel_value.trim().parse::<u32>().ok();
    let mut event = yap_events::price_event(
        preset.platform,
        preset.kind,
        Some("test".to_string()),
        preset.amount,
        configured,
    );
    event.kind = format!("{} (mock)", event.kind);
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fake_is_priced_like_the_real_thing_and_labelled_fake() {
        // The point of the presets: the same math as a simulator frame, without
        // the second app. Flat sub at 100 prices 100; 100 cheer bits at 1 bit
        // each prices 100; and both arrive wearing `(mock)`.
        let sub = super::FAKE_PRESETS
            .iter()
            .find(|p| p.kind == "sub")
            .expect("a fake sub");
        let event = super::fake_event(sub, "100");
        assert_eq!(event.words, Some(100));
        assert_eq!(event.kind, "sub (mock)");
        assert_eq!(event.user.as_deref(), Some("test"));

        let cheer = super::FAKE_PRESETS
            .iter()
            .find(|p| p.kind == "cheer")
            .expect("a fake cheer");
        let event = super::fake_event(cheer, "1");
        assert_eq!(event.words, Some(100), "100 bits × 1 word per bit");
        assert_eq!(event.kind, "cheer (mock)");
    }

    #[test]
    fn an_unpriced_fake_grants_nothing_and_says_so() {
        // Blank means off — the same rule the settings panel uses. A fake for
        // something nobody priced must land in the feed as "off", not move the
        // goal, and certain junk must behave like blank rather than a price.
        let sub = super::FAKE_PRESETS
            .iter()
            .find(|p| p.kind == "sub")
            .expect("a fake sub");

        for value in ["", "   ", "lots"] {
            let event = super::fake_event(sub, value);
            assert_eq!(event.words, None, "{value:?} must price nothing");
            assert_eq!(event.kind, "sub (mock)", "still visibly a test");
        }
    }

    #[test]
    fn every_fake_preset_prices_a_real_settings_field() {
        // A preset naming a field that does not exist would read blank forever
        // and grant nothing with no error anywhere — the decorative-control bug
        // in a new shape.
        for preset in super::FAKE_PRESETS {
            let found = crate::settings::SETTINGS_SECTIONS
                .iter()
                .flat_map(|s| s.fields.iter())
                .any(|f| f.key == preset.field);
            assert!(
                found,
                "{:?} prices {:?}, which is no field",
                preset.label, preset.field
            );
            assert!(!preset.label.is_empty(), "a preset needs a button label");
        }
    }

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
