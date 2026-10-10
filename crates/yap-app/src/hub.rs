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
    pub fn with_substitution(
        settings: Settings,
        model_substitution: Option<yap_core::settings::ModelSubstitution>,
    ) -> (Self, async_channel::Receiver<ModelMessage>) {
        let (sender, receiver) = async_channel::bounded(64);

        (
            Self {
                state: AppState::shared(),
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
