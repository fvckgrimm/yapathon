//! The seam between the UI and the background work.
//!
//! GPUI owns one thread and tokio owns the rest, and neither can await the
//! other. Recognition, the HTTP server and the Twitch socket all run on the
//! runtime; the window renders on GPUI's executor. Something has to carry state
//! between them, and this is it.
//!
//! A [`Hub`] rather than globals or a `OnceLock` of everything: it is one
//! value, constructed once at startup and passed to whatever needs it, so the
//! data flow is visible at the call site instead of being discovered at runtime.
//!
//! The channel is `async_channel` rather than `tokio::sync::mpsc` because the
//! receiving side runs on GPUI's executor. A tokio channel's `recv` panics there
//! for exactly the reason `reqwest` panics without a reactor — it requires a
//! tokio context. An async channel has no such requirement.

use std::sync::Arc;

use yap_core::settings::Settings;
use yap_server::AppState;

use crate::model::ModelMessage;

/// Forward every published snapshot to the window's message channel.
///
/// ## Why this exists
///
/// The window learns about the run through `ModelMessage::State`, and the run
/// publishes snapshots on a broadcast channel. Nothing connected the two.
///
/// The transcript and the warnings still arrived, because those are reported
/// through the hub directly rather than through the channel. So the window showed
/// a live transcript and a live warning list over a frozen count, a frozen "to
/// go", a frozen rate and a frozen timer — every number that comes from a
/// snapshot, and none of them, while the numbers pushed by hand worked fine.
/// The dock looked healthy because it polls `/api/state` rather than
/// subscribing, which is what made this read as "the window is broken" rather
/// than "nothing is forwarding".
///
/// Dropping a snapshot when the window is slow is deliberate: a queue that grows
/// while nobody is reading turns into a window that repaints a hundred times a
/// second trying to catch up, and the next tick is a second away.
fn forward_snapshots(state: &Arc<AppState>, messages: &async_channel::Sender<ModelMessage>) {
    // Subscribed here rather than inside the task: a broadcast receiver only
    // sees what was published after it subscribed, so a receiver created in the
    // spawned task could miss the first snapshot.
    let mut updates = state.subscribe();
    let messages = messages.clone();

    crate::runtime::spawn(async move {
        while let Ok(snapshot) = updates.recv().await {
            if messages.try_send(ModelMessage::State(snapshot)).is_err() {
                log::debug!("yap-a-thon: window is behind; dropped a snapshot");
            }
        }
    });
}

/// Everything the background work needs to report back.
///
/// Not `Debug`: it holds the session state, which is behind a lock and has no
/// useful printed form.
#[derive(Clone)]
pub struct Hub {
    state: Arc<AppState>,
    messages: async_channel::Sender<ModelMessage>,
    settings: Arc<Settings>,
    /// What the settings loader had to change, if anything.
    ///
    /// Carried rather than recomputed, because only the loader knows what was
    /// configured as opposed to what exists — see
    /// [`ModelSubstitution`](yap_core::settings::ModelSubstitution).
    model_substitution: Option<yap_core::settings::ModelSubstitution>,
}

impl Hub {
    /// The dock's appearance preference.
    pub fn appearance(&self) -> crate::theme::Appearance {
        crate::theme::Appearance::default()
    }

    /// Where the dock is served.
    ///
    /// Derived from the same settings the server bound with, so a message
    /// printed on shutdown names the URL that is actually listening rather than
    /// one assembled from the defaults.
    pub fn dock_url(&self) -> String {
        format!(
            "http://{}:{}/dock",
            self.settings.bind_address,
            self.settings.effective_port()
        )
    }

    /// Build a hub with its receiving end.
    ///
    /// The channel is bounded. The UI renders the newest state rather than a
    /// history, so an unbounded queue would accumulate updates nobody reads.
    /// When a window is closed and reopened the receiver is fresh anyway.
    pub fn new(settings: Settings) -> (Self, async_channel::Receiver<ModelMessage>) {
        Self::with_substitution(settings, None)
    }

    /// A hub that knows the loader substituted a model.
    ///
    /// Also starts forwarding snapshots to the message channel, which is the
    /// only reason the window knows anything at all.
    pub fn with_substitution(
        settings: Settings,
        model_substitution: Option<yap_core::settings::ModelSubstitution>,
    ) -> (Self, async_channel::Receiver<ModelMessage>) {
        let (sender, receiver) = async_channel::bounded(64);
        let state = AppState::shared();

        forward_snapshots(&state, &sender);

        (
            Self {
                state,
                messages: sender,
                settings: Arc::new(settings),
                model_substitution,
            },
            receiver,
        )
    }

    /// What the settings loader swapped, if it swapped anything.
    pub fn model_substitution(&self) -> Option<&yap_core::settings::ModelSubstitution> {
        self.model_substitution.as_ref()
    }

    /// The shared session state.
    pub fn state(&self) -> Arc<AppState> {
        Arc::clone(&self.state)
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// A sender for a background task.
    ///
    /// Cloned rather than shared: a task that outlives the hub still needs a
    /// way to report, and dropping every sender would close the channel and end
    /// the UI's update loop.
    pub fn messages(&self) -> async_channel::Sender<ModelMessage> {
        self.messages.clone()
    }

    /// Report something, without waiting.
    ///
    /// Non-blocking on purpose: this is called from the recognition task and
    /// from an audio callback's error path, and neither may stall behind a full
    /// queue. A dropped message means a missed redraw, which recovers on the
    /// next one.
    pub fn report(&self, message: ModelMessage) {
        let _ = self.messages.try_send(message);
    }

    /// Report something, awaiting if the queue is full.
    ///
    /// For a path that already has somewhere to wait and where losing the
    /// message would lose information — an error the streamer needs to see.
    pub async fn report_awaited(&self, message: ModelMessage) {
        let _ = self.messages.send(message).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yap_core::settings::Settings;

    #[tokio::test]
    async fn the_hub_delivers_snapshots_to_the_window() {
        // The bug this covers: nothing forwarded the broadcast to the window's
        // message channel, so every number derived from a snapshot stayed frozen
        // — count, to-go, rate, elapsed, from-chat — while the transcript and
        // the warnings, which are reported directly, kept working.
        //
        // A test that builds the `Hub` and asserts the message arrives is the
        // only shape that catches it: everything else in the app was already
        // correct, in isolation and on screen.
        let (hub, messages) = Hub::new(Settings::default());

        // A run has to exist: the ticker is quiet with no session, which is
        // most of the app's life.
        hub.state()
            .start_session(yap_core::session::SessionConfig::new(
                yap_core::goal::GoalMode::Fixed { words: 5_000 },
                None,
            ))
            .await
            .expect("start a run");
        hub.state().start_ticker();

        let mut seen = 0;
        let mut elapsed = std::collections::HashSet::new();

        // A few seconds of ticks.
        for _ in 0..3 {
            let message =
                tokio::time::timeout(std::time::Duration::from_millis(4_000), messages.recv())
                    .await;

            match message {
                Ok(Ok(ModelMessage::State(snapshot))) => {
                    seen += 1;
                    elapsed.insert(snapshot.elapsed_secs);
                }
                Ok(Ok(_)) => {}
                Ok(Err(_)) => panic!("the message channel closed"),
                Err(_) => panic!("no snapshot arrived within four seconds"),
            }
        }

        assert!(
            seen >= 2,
            "the window should be receiving snapshots; got {seen}",
        );
        assert!(
            elapsed.len() >= 2,
            "the snapshots carried a moving clock, not a frozen one: {elapsed:?}",
        );
    }

    /// A port nothing is listening on.
    ///
    /// Not port 0: `Settings::effective_port` maps that to the default on
    /// purpose, so a test asking for any free port would collide with a real
    /// instance already on 8973.
    async fn free_port() -> u16 {
        tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port")
            .local_addr()
            .expect("read the assigned port")
            .port()
    }

    #[tokio::test]
    async fn the_receiver_is_connected() {
        let (hub, receiver) = Hub::new(Settings::default());

        hub.report(ModelMessage::Listening(true));

        // The point of the whole type: a message from a tokio task reaching a
        // consumer that is not on a tokio executor.
        let message = receiver.recv().await.expect("a message");
        assert!(matches!(message, ModelMessage::Listening(true)));
    }

    #[tokio::test]
    async fn a_cloned_sender_still_works() {
        // A long-lived task holding a clone must not be able to end the UI's
        // update loop by dropping the original.
        let (hub, receiver) = Hub::new(Settings::default());
        let clone = hub.messages();

        drop(hub.state.clone());
        clone.try_send(ModelMessage::Behind(true)).expect("sent");

        assert!(matches!(
            receiver.recv().await.expect("a message"),
            ModelMessage::Behind(true)
        ));
    }

    #[test]
    fn reporting_never_blocks_the_caller() {
        // An audio callback's error path calls this. Blocking there would
        // glitch the stream.
        let (hub, _receiver) = Hub::new(Settings::default());
        for i in 0..200 {
            hub.report(ModelMessage::Warning(format!("warning {i}")));
        }
        // Reaching here without blocking is the assertion.
    }

    #[test]
    fn the_hub_owns_one_state() {
        let (hub, _receiver) = Hub::new(Settings::default());
        assert!(Arc::ptr_eq(&hub.state(), &hub.state()));
    }

    #[tokio::test]
    async fn the_server_and_the_window_share_one_session() {
        // The bug this guards: `start_server` used to build its own `AppState`,
        // so the window and OBS were looking at two different sessions. Every
        // symptom read as a rendering fault — starting a yapathon in the dock did
        // nothing to the window's counter — and nothing errored, because both
        // halves were individually healthy.
        let (hub, _receiver) = Hub::new(Settings {
            port: free_port().await,
            ..Settings::default()
        });

        let server = crate::app::start_server(hub.settings(), hub.state())
            .await
            .expect("bind");

        assert!(
            Arc::ptr_eq(server.state(), &hub.state()),
            "the server and the window must share one session"
        );
    }

    #[tokio::test]
    async fn a_session_started_in_one_place_is_visible_through_the_api() {
        // The observable consequence, asserted through the path the overlay
        // reads: a yapathon started from the window must be the same yapathon
        // the server reports.
        let (hub, _receiver) = Hub::new(Settings::default());
        let state = hub.state();

        crate::session::start(&state, crate::session::StartRequest::extend(1_234, None))
            .await
            .expect("start");

        let snapshot = state.snapshot().await.expect("a snapshot");
        assert_eq!(snapshot.target, 1_234);
    }
}
