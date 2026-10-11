//! Startup: the window, the server, and everything running behind it.
//!
//! The order is load-bearing. The server binds first so the OBS URL exists the
//! moment the window opens. Recognition starts last, because a missing model
//! must not stop the app from opening: a streamer who has not downloaded
//! whisper yet should still get a window, a dock they can read, and a clear
//! message about why the count is sitting at zero.

use std::sync::Arc;

use async_trait::async_trait;
use gpui::px;
use yap_core::settings::Settings;
use yap_events::astro::{AstroAccount, AstroPlatform};
use yap_events::{ChatPlatform, EventSink, TwitchPlatform, TwitchPricing};
use yap_server::AppState;
use yap_voice::pipeline::WordSink;

use crate::hub::Hub;
use crate::model::{ModelMessage, SessionEntity, TranscriptLine};
use crate::runtime;
use crate::theme::Appearance;
use crate::view::YapView;

/// The app id used for the window and by the platform for its identity.
pub const APP_ID: &str = "com.yapathon.app";

/// Open the main window.
///
/// # Errors
/// Propagates window creation failures. Past this point there is no window to
/// report into, so the caller prints to stderr and exits.
pub fn open_main_window(
    hub: Hub,
    messages: async_channel::Receiver<ModelMessage>,
    server_address: std::net::SocketAddr,
    settings: yap_core::settings::Settings,
    settings_path: std::path::PathBuf,
    cx: &mut gpui_kit::App,
) -> anyhow::Result<()> {
    let (width, height) = initial_size(cx);
    log::info!("yap-a-thon: opening at {width}x{height}");

    let overlay_url = format!("http://{server_address}/overlay");

    let options = gpui::WindowOptions {
        app_id: Some(APP_ID.to_owned()),
        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
            gpui::point(px(0.0), px(0.0)),
            gpui::size(px(width), px(height)),
        ))),
        ..Default::default()
    };

    // Dark unless the preference says otherwise, applied before the window so
    // the first frame is already the right colour rather than visibly changing
    // one frame later.
    apply_appearance(appearance_from(&settings.appearance), cx);
    apply_accent(&settings.accent);

    let session = SessionEntity::new(hub.state(), messages, overlay_url, cx);

    // The view is built inside the window builder, not here: the settings panel
    // constructs real input states, and those need a `Window`. The builder is
    // the first place one exists.
    //
    // `open_window` wraps the returned entity in `Root`, which owns overlays —
    // wrapping again would double them.
    gpui_kit::open_window(options, cx, move |window, cx| {
        YapView::new(
            session.clone(),
            settings.clone(),
            settings_path.clone(),
            window,
            cx,
        )
    })?;

    // Background work starts only after the window exists, so nothing that can
    // fail keeps the app from appearing.
    start_listening(&hub);
    start_twitch(&hub);
    start_astro(&hub);

    Ok(())
}

/// Start the microphone and the recogniser, and keep them in step with settings.
///
/// # Why this is a supervisor rather than a straight line
///
/// The engine used to be built once, at startup, and the listener parked on
/// `pending()` for the life of the process. That made a model change need a
/// restart, and the settings panel said so — which was honest, and also the
/// reason the answer to "can I switch models without losing my place" was no.
///
/// So the microphone is now supervised: it watches for a capture configuration
/// that differs from the one it loaded, stops, rebuilds, and starts again.
///
/// Words spoken during the swap are lost, and that is unavoidable — a model is
/// hundreds of megabytes read into memory and the microphone cannot run through
/// it. It is why this happens when the settings are *saved* rather than when a
/// run starts: nobody is mid-count at that moment unless they are.
///
/// A run in progress is not a reason to refuse. The streamer has chosen to swap
/// and would rather lose the last word than have the change silently ignored
/// until the next launch, which is the alternative this replaces.
fn start_listening(hub: &Hub) {
    let hub = hub.clone();
    let state = hub.state();

    // Watched before the first load, so a settings change made in the moment
    // between this task starting and the microphone opening is not missed — the
    // loop below compares against what it is running, so a notice that arrived
    // early simply finds a difference and rebuilds.
    let Some(mut reloads) = state.watch_capture() else {
        log::warn!("yap-a-thon: no capture channel; the microphone will not reload");
        return;
    };

    runtime::spawn(async move {
        let sink: Arc<dyn WordSink> = Arc::new(SessionSink { hub: hub.clone() });

        // The configuration this loop is currently running, seeded with what
        // the hub loaded at startup. Compared against rather than tracked, so a
        // change that happened before this task first ran is still noticed.
        let mut running = crate::hub::CaptureConfig::of(hub.settings());

        loop {
            // Reported per load, not once: a reload that silently used the old
            // model would be indistinguishable from a reload that worked.
            report_model_in_use(&hub, &running.engine);

            let listener = match open_listener(&hub, &running, sink.clone()).await {
                Some(listener) => listener,
                None => {
                    // The build failed. Park until the settings change, rather
                    // than spinning: retrying a missing model every second
                    // would fill the log and the warnings list with the same
                    // line, and a warning that repeats forever is one nobody
                    // reads.
                    log::warn!("yap-a-thon: recogniser not available, waiting for a change");
                    if wait_for_change(&hub, &mut reloads, &running).await {
                        continue;
                    }
                    return;
                }
            };

            hub.state().set_listening(true).await;
            hub.report(ModelMessage::Listening(true));

            if !wait_for_change(&hub, &mut reloads, &running).await {
                return;
            }

            // Drain before rebuilding, so the last second of speech is counted
            // against the run that was happening rather than the next one.
            listener.stop().await;
            hub.state().set_listening(false).await;
            hub.report(ModelMessage::Listening(false));

            let next = crate::hub::CaptureConfig::of(hub.settings());
            if next == running {
                // Woken by a write that did not change what is running. Nothing
                // to rebuild, and stopping the microphone for it would cost a
                // word for no reason.
                log::debug!("yap-a-thon: capture settings rewritten but unchanged");
                continue;
            }

            log::info!("yap-a-thon: reloading the recogniser");
            running = next;
        }
    });
}

/// Wait until the capture configuration differs from the running one.
///
/// `false` means the channel closed, which means the app is shutting down.
///
/// The hub is asked rather than the channel carrying the value, because the
/// notice is a unit type on purpose — see [`yap_server::state::CaptureNotice`].
/// Comparing the two is what makes a save that rewrites the settings file
/// without changing the microphone a no-op instead of a needless rebuild.
async fn wait_for_change(
    hub: &Hub,
    reloads: &mut tokio::sync::watch::Receiver<yap_server::state::CaptureNotice>,
    running: &crate::hub::CaptureConfig,
) -> bool {
    // A closed channel means the app is shutting down. Continuing would spin on
    // a configuration that will never change again.
    wait_for_capture_change(reloads, hub.settings(), running).await
}

/// The wait itself, over plain settings so it can be tested without a hub.
async fn wait_for_capture_change(
    reloads: &mut tokio::sync::watch::Receiver<yap_server::state::CaptureNotice>,
    settings: &yap_core::settings::Settings,
    running: &crate::hub::CaptureConfig,
) -> bool {
    loop {
        if crate::hub::CaptureConfig::of(settings) != *running {
            return true;
        }

        if reloads.changed().await.is_err() {
            return false;
        }
    }
}

/// Build the engine and open the microphone for one configuration.
///
/// `None` means the build failed and the caller should wait; the reason has
/// already been reported, because it is the same reason a streamer needs to see
/// whether it happened at startup or after a reload.
async fn open_listener(
    hub: &Hub,
    config: &crate::hub::CaptureConfig,
    sink: Arc<dyn WordSink>,
) -> Option<yap_voice::pipeline::Listener> {
    let engine = match yap_voice::build_engine(&config.engine).await {
        // `build_engine` returns a `Box<dyn TranscriptionEngine>`; this converts
        // it to the `Arc` the listener shares, so a local engine and a cloud one
        // are interchangeable to everything above this line.
        Ok(engine) => Arc::from(engine) as Arc<dyn yap_core::engine::TranscriptionEngine>,
        Err(error) => {
            // The message names the directory to put a model in. "no model"
            // with no path is what sends someone hunting their filesystem.
            let message = match &config.engine {
                yap_core::settings::EngineChoice::Local { .. } => format!(
                    "{error}\nPut a ggml model in {} — the Settings tab lists any it finds \
                     there.",
                    yap_core::settings::models_dir().display()
                ),
                // A cloud engine's failure is about the key or the network,
                // and neither is helped by advice about model files.
                yap_core::settings::EngineChoice::Groq { .. } => error.to_string(),
            };

            hub.report_awaited(ModelMessage::Warning(message)).await;
            return None;
        }
    };

    // The device name is already trimmed by `CaptureConfig`, and an empty one
    // means "the system default" rather than a device that does not exist.
    if config.input_device.is_empty() {
        log::info!("yap-a-thon: opening the default microphone");
    } else {
        log::info!("yap-a-thon: opening microphone {:?}", config.input_device);
    }

    let started = if config.input_device.is_empty() {
        yap_voice::pipeline::Listener::start(engine, config.vad, sink).await
    } else {
        yap_voice::pipeline::Listener::start_named(
            config.input_device.clone(),
            engine,
            config.vad,
            sink,
        )
        .await
    };

    match started {
        Ok(listener) => {
            log::info!("yap-a-thon: microphone open");
            Some(listener)
        }
        Err(error) => {
            hub.report_awaited(ModelMessage::Warning(format!(
                "microphone unavailable: {error:#}"
            )))
            .await;
            None
        }
    }
}

/// Say which model is in use, and say so when it is not the one that was asked for.
///
/// Split out because it now runs on every load rather than once at startup, and
/// because the substitution is decided by the loader, which is the only place
/// that knows both what was configured and what exists. Working it out here by
/// comparing against the built-in default name instead warned on every install
/// that was not running that exact file, which is every install — and a warning
/// that is always true is one a streamer learns to ignore, including the time
/// it is telling the truth.
fn report_model_in_use(hub: &Hub, engine: &yap_core::settings::EngineChoice) {
    if !matches!(engine, yap_core::settings::EngineChoice::Local { .. }) {
        return;
    }

    match hub.model_substitution_for(engine) {
        Some(substitution) => {
            hub.report(ModelMessage::Warning(format!(
                "using {} — no model at {}",
                substitution.using.display(),
                substitution.wanted.display(),
            )));
        }
        None => log::info!("yap-a-thon: using the configured model"),
    }
}

/// Start the Twitch connection, if one is configured.
///
/// Absent credentials is not an error: chat commands work without them, and a
/// streamer can run a whole yapathon that way.
fn start_twitch(hub: &Hub) {
    let (Some(channel), Some(service), Some(client_id)) = (
        hub.settings().twitch_channel.clone(),
        hub.settings().twitch_credential.clone(),
        hub.settings().twitch_client_id.clone(),
    ) else {
        // All three or none: EventSub refuses the socket without a client id
        // and silently receives nothing without a token, so a partial config is
        // treated as no config rather than half-working.
        log::info!(
            "yap-a-thon: twitch is not fully configured (channel, credential and \
             client id are all needed); chat stays manual"
        );
        return;
    };

    let hub = hub.clone();

    runtime::spawn(async move {
        // Pricing comes from the settings file, not from the platform's
        // built-in defaults. It used to, which meant the Events section of the
        // settings panel was editing numbers nothing read — a control that
        // shows a value and changes nothing, which is the bug this project has
        // been repeatedly bitten by.
        let pricing = twitch_pricing(hub.settings());

        let platform = match TwitchPlatform::from_keyring(channel, &service, &client_id, pricing) {
            Ok(platform) => platform,
            Err(error) => {
                hub.report_awaited(ModelMessage::Warning(format!(
                    "twitch not connected: {error:#}"
                )))
                .await;
                return;
            }
        };

        let sink: Arc<dyn EventSink> = Arc::new(StateEventSink { hub: hub.clone() });

        if let Err(error) = platform.run(sink).await {
            hub.report(ModelMessage::Warning(format!("twitch stopped: {error:#}")));
        }
    });
}

/// Start StreamElements Astro, if a token is configured.
///
/// Off unless a token is present. That is the whole point of it being
/// opt-in: it puts a third party between the streamer and their events, which
/// is a trade rather than a free win, and the streamer should make it.
///
/// One connection per token, because a token is scoped to one platform account
/// and a cross-streaming streamer has more than one.
fn start_astro(hub: &Hub) {
    // Read here rather than lazily on the first simulated event, so the warning
    // that this build is counting fake cheers appears at launch — where it can
    // still stop the stream — rather than after one has already been counted.
    if !yap_events::astro::mock_events_allowed() {
        return;
    }

    // Astro names events `<platform>:<kind>`, the same shape EventSub uses, so
    // the prices a streamer already set carry over rather than silently
    // re-pricing itself to nothing on the day they switch.
    let pricing = astro_pricing(hub.settings());
    let twitch_native = native_twitch_configured(hub.settings());
    let hub = hub.clone();

    // **Nothing that can block happens on this thread.**
    //
    // `astro_accounts` reads the credential store, and a *locked* keyring makes
    // the client wait for an unlock prompt that a desktop app cannot answer —
    // it does not error, it hangs, for as long as the keyring stays locked.
    //
    // This function runs while the window is being constructed, so blocking here
    // means the window is never presented: the app looks like it failed to
    // launch, with no error anywhere. It did exactly that, and the comment above
    // `start_astro` promised the opposite.
    //
    // The same reading is already on a background task in `start_twitch`, which
    // is why a locked keyring hung Astro and not Twitch.
    runtime::spawn(async move {
        let sources = reconcile_sources(astro_accounts(), twitch_native);

        for platform in &sources.dropped {
            // Named precisely, because "something was ignored" is the kind of
            // message that gets read once and then not again.
            log::warn!(
                "astro: not connecting the {platform} account — twitch is also set up \
                 natively, and running both would count every cheer, sub and raid twice. \
                 Clear the native Twitch settings to use Astro for {platform} instead"
            );
            hub.report(ModelMessage::Warning(format!(
                "{platform}: native Twitch is configured, so Astro is not connecting it too — \
                 running both would count events twice"
            )));
        }

        let accounts = sources.kept;
        if accounts.is_empty() {
            log::info!("astro: no tokens configured, not connecting");
            return;
        }

        let platform = AstroPlatform::new(accounts, pricing);
        let sink: Arc<dyn EventSink> = Arc::new(StateEventSink { hub });

        log::info!("astro: connecting");
        if let Err(error) = platform.run(sink).await {
            log::warn!("astro stopped: {error:#}");
        }
    });
}

/// The Astro accounts configured, from the credential store.
///
/// One per platform, each in its own keyring entry, so the panel can say
/// whether each is set and clearing one leaves the other alone.
///
/// Missing means Astro is not used, which is the default and the point: it puts
/// a third party between the streamer and their events, so it is a choice.
///
/// The platform a token covers is **not** read from the settings — it comes from
/// every event's `provider` field. Putting a token in the wrong slot therefore
/// costs nothing but a misleading label; the log says which platform actually
/// arrived, so a mistake is visible rather than silent.
pub fn astro_accounts() -> Vec<AstroAccount> {
    let mut accounts = Vec::new();
    let service = yap_core::settings::credential_name("astro");

    // A missing keyring is not an error. It means this machine cannot store a
    // credential, so there is nothing to connect with — the same reasoning that
    // stops the settings panel offering to accept a token it cannot keep.
    for platform in ["youtube", "twitch"] {
        // No token means that platform is simply not used.
        let Ok(Some(token)) = yap_core::secrets::get(&service, platform) else {
            continue;
        };

        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        accounts.push(AstroAccount {
            token: token.to_string(),
            platform,
        });
    }

    accounts
}

/// Whether the native Twitch connection is configured.
///
/// All three settings or none, which is what `start_twitch` requires and what
/// this mirrors.
/// The prices Astro events are awarded with.
///
/// The whole `<platform>:<kind>` map, unchanged.
///
/// Astro names an event by the provider that sent it, so `twitch:cheer` arriving
/// through Astro means exactly what it means arriving through EventSub and must
/// carry exactly the same price.
///
/// Filtering by platform here would look tidier and would be wrong.
/// `Settings::pricing_for` strips the prefix, so a filtered map is keyed `cheer`
/// while the lookup is by `twitch:cheer` — every price misses, every event from
/// every platform silently grants nothing, and the overlay shows a number that
/// never moves. Which is exactly what happened.
///
/// Named rather than inlined at the call site for the same reason
/// [`twitch_pricing`] is: a test that builds the pricing itself passes whether
/// or not the call site uses the same keys, which is how this stayed hidden
/// while everything looked connected and nothing was priced.
pub fn astro_pricing(
    settings: &yap_core::settings::Settings,
) -> std::collections::BTreeMap<String, u32> {
    settings.event_words.clone()
}

pub fn native_twitch_configured(settings: &yap_core::settings::Settings) -> bool {
    settings.twitch_channel.is_some()
        && settings.twitch_credential.is_some()
        && settings.twitch_client_id.is_some()
}

/// The result of deciding which sources may run at once.
#[derive(Debug, Default)]
pub struct Sources {
    /// Accounts to connect.
    pub kept: Vec<AstroAccount>,
    /// Platforms dropped, because a native connection already covers them.
    pub dropped: Vec<&'static str>,
}

/// Decide which Astro accounts may run alongside native connections.
///
/// A platform covered by both is a platform counted twice. Native EventSub and
/// Astro both report `twitch:cheer` with the same bits, both reach the same
/// `StateEventSink`, and nothing downstream can tell them apart — so a streamer
/// who left both configured would watch their goal climb at double speed with
/// nothing wrong on screen. That is the worst kind of bug this project has:
/// no error, no warning, just a number that is wrong.
///
/// Native wins, for a streamer cross-streaming: EventSub is direct from Twitch
/// and already configured, while Astro is worth adding for the platform native
/// cannot cover. Someone who wants Astro for everything simply clears the native
/// settings, which is one field each.
///
/// Pure, so it can be tested — the alternative is a rule that only exists in a
/// branch nobody exercises.
pub fn reconcile_sources(accounts: Vec<AstroAccount>, native_twitch: bool) -> Sources {
    let mut kept = Vec::new();
    let mut dropped = Vec::new();

    for account in accounts {
        if account.platform == "twitch" && native_twitch {
            dropped.push("twitch");
        } else {
            kept.push(account);
        }
    }

    Sources { kept, dropped }
}

/// The pricing Twitch events are awarded with.
///
/// Named rather than inlined at the call site, because it used to be inlined —
/// as `TwitchPricing::default()`, a set of hardcoded numbers. That made the
/// Events section of the settings panel decorative: it read and wrote
/// `settings.event_words` faithfully and nothing ever read it back, so changing a
/// price and pressing Save did nothing and reported nothing.
///
/// It is a function so there is a thing to test. A test that builds the pricing
/// itself would pass whether or not the call site used it, which is exactly the
/// gap that let the original bug through.
pub fn twitch_pricing(settings: &yap_core::settings::Settings) -> TwitchPricing {
    TwitchPricing::from_words(settings.pricing_for("twitch"))
}

/// Recognised words, into the session.
struct SessionSink {
    hub: Hub,
}

#[async_trait]
impl WordSink for SessionSink {
    fn on_transcript(&self, transcript: &yap_core::engine::Transcript) {
        let hub = self.hub.clone();
        let text = transcript.text.clone();

        // `on_transcript` runs on the recognition task. Recording takes the
        // session's write lock, so it is moved onto its own task rather than
        // blocking the pipeline behind it.
        runtime::spawn(async move {
            let words = hub.state().record_words(&text, 0, 0).await;
            if words == 0 {
                return;
            }

            let transcript = hub.state().transcript().await;
            hub.report(ModelMessage::Transcript(
                transcript
                    .into_iter()
                    .map(|(_, words, text, voided)| TranscriptLine {
                        words,
                        text,
                        voided,
                    })
                    .collect(),
            ));
        });
    }

    fn on_error(&self, utterance_ms: u64, error: &yap_core::engine::EngineError) {
        // A failed utterance does not fail a yapathon, but it must be visible:
        // silently dropping words leaves the final count wrong with no
        // explanation for the shortfall.
        self.hub
            .report(ModelMessage::Warning(format!("{utterance_ms}ms: {error}")));
    }
}

/// Platform events, into the session and the feed.
struct StateEventSink {
    hub: Hub,
}

impl EventSink for StateEventSink {
    fn on_event(&self, event: yap_core::goal::Event) {
        let hub = self.hub.clone();

        runtime::spawn(async move {
            let granted = hub.state().apply_event(event.clone()).await;
            if granted > 0 {
                hub.report(ModelMessage::Event(event));
            }
        });
    }
}

/// Apply the appearance preference to the theme.
///
/// Dark by default. `Auto` resolves against the desktop and then tracks it, so
/// a streamer who never touches the setting gets the right panel at 8am and at
/// 9pm without a restart.
fn apply_appearance(appearance: Appearance, cx: &mut gpui_kit::App) {
    use gpui_kit::component::Theme;

    // No reliable way to ask the desktop from here without pulling in another
    // crate, and this app is dark-first anyway. `Auto` therefore resolves dark
    // unless a future platform check says otherwise, which is the honest
    // behaviour rather than guessing light and being wrong on most desktops.
    let mode = appearance.resolve(true);

    Theme::change(mode, None, cx);
}

/// Apply the settings that decide how the window looks.
///
/// Separate from [`apply_appearance`] because the accent is ours rather than
/// the component theme's: `Theme::accent` reads a process-wide value, so it has
/// to be set before anything renders, and `Theme::change` has no idea it
/// exists.
fn apply_accent(accent: &str) {
    crate::theme::set_accent(crate::theme::Accent::from_id(accent));
}

/// The appearance a settings value names.
///
/// Unknown names fall back rather than failing: the value comes out of a file a
/// person can edit, and a typo should cost them their theme, not their app.
pub(crate) fn appearance_from(name: &str) -> Appearance {
    match name.trim().to_lowercase().as_str() {
        "light" => Appearance::Light,
        "dark" => Appearance::Dark,
        _ => Appearance::Auto,
    }
}

/// A window that fits the display it opens on.
///
/// A fixed size larger than a small laptop opens partly off-screen, where there
/// is no way to reach it and nothing indicates it is there — worse than a small
/// window on any machine.
fn initial_size(cx: &mut gpui_kit::App) -> (f32, f32) {
    const DEFAULT_WIDTH: f32 = 560.0;
    const DEFAULT_HEIGHT: f32 = 860.0;
    const MIN_WIDTH: f32 = 360.0;

    let Some(display) = cx.displays().first().cloned() else {
        return (DEFAULT_WIDTH, DEFAULT_HEIGHT);
    };

    let bounds = display.bounds();
    let width = DEFAULT_WIDTH
        .min(f32::from(bounds.size.width) * 0.9)
        .max(MIN_WIDTH);
    let height = DEFAULT_HEIGHT.min(f32::from(bounds.size.height) * 0.9);

    (width, height)
}

/// Bind the local server OBS reads.
///
/// # Errors
/// Propagates bind failures. The address is reported exactly, because the
/// streamer has to type it into OBS.
pub async fn start_server(
    settings: &Settings,
    state: Arc<AppState>,
) -> anyhow::Result<yap_server::Server> {
    // The caller's state, not a fresh one.
    //
    // This used to call `AppState::shared()` itself, which handed the server a
    // *different* session than the window had. Every symptom looked like a
    // rendering problem: starting a yapathon in the dock did nothing to the
    // window's counter, `listening` never turned true, and the overlay showed a
    // second empty session. Nothing errored, because both halves were perfectly
    // healthy — they just were not the same half.
    //
    // The overlay theme goes in here for the same reason: it is a setting the
    // served page needs, and the page cannot read the file per request.
    state.set_overlay_theme(&settings.overlay_theme);
    state.set_overlay_style(&settings.overlay_style);

    // The clock, so elapsed time and words-per-minute move even while the
    // streamer is not talking. Started with the server rather than the window so
    // the overlay and the dock get it too, including when no window exists.
    state.start_ticker();

    let server =
        yap_server::Server::bind(&settings.bind_address, settings.effective_port(), state).await?;

    log::info!("yap-a-thon: dock at {}", server.dock_url());
    log::info!("yap-a-thon: overlay at {}", server.overlay_url());

    Ok(server)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelMessage, SessionEntity};
    use yap_core::goal::GoalMode;
    use yap_core::session::{Session, SessionConfig};

    /// Holds a session entity alive for a test about the entity rather than the
    /// window on top of it.
    ///
    /// Needs to be a window root because `add_window_view` hands the context it
    /// constructs the entity with, and a bare `Entity` has no context to
    /// construct one in.
    struct Holder {
        entity: gpui::Entity<SessionEntity>,
    }

    impl gpui::Render for Holder {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    #[gpui_kit::test]
    fn a_clock_tick_reaches_the_window(cx: &mut gpui_kit::TestAppContext) {
        // The other half of the frozen-stats fix.
        //
        // The ticker publishes a snapshot every second, and the entity folds it
        // in — but a folded-in model is not a repainted window. The view
        // subscribes to the entity, and repainting only happens if the entity
        // says so. When the model changed silently the window sat at whatever
        // it last drew, which for elapsed time meant the timer never moved.
        //
        // So this asserts the notification, not the value: a model that quietly
        // holds the right number looks exactly like a frozen one from in here.
        use std::cell::Cell;
        use std::rc::Rc;

        let (_sender, receiver) = async_channel::unbounded::<ModelMessage>();
        let state = std::sync::Arc::new(yap_server::AppState::shared());

        cx.update(gpui_kit::init);
        let (holder, cx) = cx.add_window_view({
            let state = state.clone();
            move |_window, cx| Holder {
                entity: SessionEntity::new(
                    std::sync::Arc::clone(&state),
                    receiver,
                    "http://127.0.0.1:8973/overlay".to_string(),
                    cx,
                ),
            }
        });
        let entity = cx.update(|_window, cx| holder.read(cx).entity.clone());

        let repaints = Rc::new(Cell::new(0usize));
        let _observing = cx.update(|_window, cx| {
            let repaints = Rc::clone(&repaints);
            cx.observe(&entity, move |_entity, _cx| {
                repaints.set(repaints.get() + 1);
            })
        });

        // Two snapshots a second apart, which is exactly what the ticker sends.
        let mut session = Session::new(SessionConfig::new(GoalMode::Fixed { words: 5000 }, None));
        session.start();

        for _ in 0..2 {
            _sender
                .send_blocking(ModelMessage::State(session.snapshot()))
                .expect("the entity is listening");
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(1_100));
        }

        assert!(
            repaints.get() >= 2,
            "a moving clock must repaint the window; got {} repaints",
            repaints.get(),
        );
    }

    #[test]
    fn the_appearance_setting_reaches_the_theme() {
        // A typo in the file must cost the theme, not the app: `Appearance` has
        // no error case, so anything unrecognised resolves to following the
        // desktop, which is what a person who never touched the setting gets.
        assert_eq!(appearance_from("light"), Appearance::Light);
        assert_eq!(appearance_from("dark"), Appearance::Dark);
        assert_eq!(appearance_from("auto"), Appearance::Auto);
        assert_eq!(appearance_from("  LIGHT "), Appearance::Light);

        for nonsense in ["", "darkk", "true", "midnight"] {
            assert_eq!(
                appearance_from(nonsense),
                Appearance::Auto,
                "{nonsense:?} should fall back rather than fail",
            );
        }
    }

    #[test]
    fn twitch_pricing_comes_from_the_settings_file() {
        // The wiring, not just the pieces. This used to be
        // `TwitchPricing::default()` at the call site, which is why the Events
        // settings section could be edited all day and change nothing.
        let settings = yap_core::settings::Settings {
            event_words: [
                ("twitch:cheer".to_string(), 1),
                ("twitch:gift_sub".to_string(), 7),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };

        let pricing = twitch_pricing(&settings);
        assert_eq!(pricing.get("gift_sub"), Some(7));

        // And it must differ from the hardcoded defaults, or the assertion above
        // would also pass with the old code — which is 50, so it would not, but
        // saying so here is cheaper than discovering it later when a default
        // changes to match a test.
        assert_ne!(
            pricing.get("gift_sub"),
            TwitchPricing::default().get("gift_sub"),
        );

        let event = yap_events::price_event(
            "twitch",
            "gift_sub",
            None,
            Some(100.0),
            pricing.get("gift_sub"),
        );
        assert_eq!(event.words, Some(700));
    }

    #[test]
    fn a_priced_platform_does_not_leak_into_another() {
        // Only relevant once YouTube exists, but it is the property that makes
        // adding a platform safe, so it is pinned now while it is cheap.
        let settings = yap_core::settings::Settings {
            event_words: [("twitch:cheer".to_string(), 1)].into_iter().collect(),
            ..Default::default()
        };

        let twitch = twitch_pricing(&settings);
        assert_eq!(twitch.words.len(), 1);
        assert!(
            !twitch.words.contains_key("superchat"),
            "twitch pricing must only contain twitch kinds",
        );
    }

    /// A port nothing is listening on.
    ///
    /// Port 0 cannot be used for this: [`Settings::effective_port`] maps a zero
    /// port to the default, deliberately, so that a hand-edited file saying
    /// `port = 0` binds the documented port rather than an arbitrary one. That
    /// means a test asking for "any free port" gets 8973 and collides with
    /// whatever is running.
    async fn free_port() -> u16 {
        tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port")
            .local_addr()
            .expect("read the assigned port")
            .port()
    }

    #[test]
    fn the_app_id_is_stable() {
        // The platform uses it for window identity and the settings location, so
        // changing it orphans every streamer's saved configuration.
        assert_eq!(APP_ID, "com.yapathon.app");
    }

    #[tokio::test]
    async fn the_server_binds_on_loopback_from_settings() {
        let settings = Settings {
            port: free_port().await,
            ..Settings::default()
        };

        let server = start_server(&settings, AppState::shared())
            .await
            .expect("bind");
        assert!(server.address().ip().is_loopback());
        assert_eq!(server.address().port(), settings.port);
    }

    #[test]
    fn a_platform_covered_twice_is_connected_once() {
        // The bug: nothing stopped both connections running. Native EventSub and
        // Astro both report `twitch:cheer` with the same bits, both reach the
        // same sink, and nothing downstream can tell them apart — so the goal
        // would climb at double speed with no error and nothing wrong on
        // screen. The worst kind of bug, because it is invisible and it is a
        // number.
        let accounts = vec![
            AstroAccount {
                token: "t".into(),
                platform: "twitch",
            },
            AstroAccount {
                token: "y".into(),
                platform: "youtube",
            },
        ];

        // Both configured: Twitch is already covered natively.
        let both = reconcile_sources(accounts.clone(), true);

        assert_eq!(
            both.kept.iter().map(|a| a.platform).collect::<Vec<_>>(),
            vec!["youtube"],
            "youtube is not covered natively, so it must still connect",
        );
        assert_eq!(
            both.dropped,
            vec!["twitch"],
            "twitch would have been counted twice"
        );

        // Native not configured: nothing to conflict with.
        let alone = reconcile_sources(accounts.clone(), false);
        assert_eq!(alone.kept.len(), 2, "both platforms connect");
        assert!(alone.dropped.is_empty());
    }

    #[test]
    fn dropping_a_platform_is_a_connection_decision_not_a_deletion() {
        // `reconcile_sources` takes the accounts by value and hands back the
        // ones it did not use. It must not delete anything: a streamer who
        // clears the native Twitch settings to resolve a conflict should find
        // their Astro token still there next launch, not have to copy it again.
        //
        // Tested on the returned accounts rather than by reading the keyring,
        // because a test that reads a real store is a test that depends on what
        // happens to be configured on the machine running it.
        let accounts = vec![
            AstroAccount {
                token: "twitch-token".into(),
                platform: "twitch",
            },
            AstroAccount {
                token: "youtube-token".into(),
                platform: "youtube",
            },
        ];

        let conflicted = reconcile_sources(accounts.clone(), true);
        assert_eq!(conflicted.dropped, vec!["twitch"]);
        assert_eq!(
            conflicted.kept.len(),
            1,
            "the token is still held, just not connected"
        );

        // The token survives the decision, byte for byte.
        assert_eq!(conflicted.kept[0].token, "youtube-token");

        // And nothing about the dropped one is needed to get it back, because
        // the token was never read from — it was passed in.
        let resolved = reconcile_sources(accounts, false);
        assert_eq!(resolved.kept.len(), 2);
        assert!(resolved.dropped.is_empty());
    }

    #[test]
    fn native_twitch_needs_all_three_settings_and_not_two_of_three() {
        // Mirrors what `start_twitch` requires. A partial config must not count
        // as "native is on" — otherwise a streamer with a leftover channel name
        // would silently lose their Astro Twitch connection to a conflict with
        // something that was never going to connect anyway.
        //
        // `twitch_credential` defaults to an entry *name*, so it is cleared
        // explicitly in the partial cases. Without that, "one of three" is
        // secretly "two of three" and the test passes for the wrong reason.
        // Built fresh each time: `Settings` owns strings, so a shared base
        // would be moved into the first case and unavailable to the rest.
        let unset = || Settings {
            twitch_credential: None,
            ..Settings::default()
        };

        assert!(
            !native_twitch_configured(&Settings::default()),
            "an entry name with no channel and no client id is not a connection",
        );

        assert!(
            !native_twitch_configured(&Settings {
                twitch_channel: Some("someone".into()),
                ..unset()
            }),
            "one of three is not configured",
        );

        assert!(
            !native_twitch_configured(&Settings {
                twitch_channel: Some("someone".into()),
                twitch_client_id: Some("id".into()),
                ..unset()
            }),
            "two of three is still not configured",
        );

        assert!(
            native_twitch_configured(&Settings {
                twitch_channel: Some("someone".into()),
                twitch_client_id: Some("id".into()),
                twitch_credential: Some("yap-a-thon/twitch".into()),
                ..Settings::default()
            }),
            "all three is configured",
        );
    }

    #[test]
    fn a_price_in_the_settings_file_actually_prices_an_astro_event() {
        // The bug that cost a day.
        //
        // `start_astro` built its pricing with `Settings::pricing_for`, which
        // strips the platform prefix, and `AstroPlatform` looked events up by the
        // full `twitch:cheer`. Every lookup missed, every event from every
        // platform was silently unpriced, and the overlay showed a number that
        // never moved — while the log cheerfully said it was "receiving twitch
        // events" and the payload tests all passed.
        //
        // The gap is exactly the seam: nothing crossed from the settings file
        // into an actual priced event. Every other test built its own pricing,
        // so every other test agreed with itself.
        let mut settings = Settings::default();
        settings.event_words.insert("twitch:cheer".into(), 1);
        settings.event_words.insert("youtube:superchat".into(), 10);

        let platform = AstroPlatform::new(Vec::new(), astro_pricing(&settings));

        let cheer = r#"{"type":"message","topic":"channel.activities",
          "data":{"type":"cheer","provider":"twitch",
            "data":{"displayName":"Someone","amount":2500}}}"#;

        let event = priced_event(&platform, cheer);
        assert_eq!(
            event.words,
            Some(2500),
            "a price of 1 per bit must actually reach a cheer",
        );

        let superchat = r#"{"type":"message","topic":"channel.activities",
          "data":{"type":"superchat","provider":"youtube",
            "data":{"displayName":"Viewer","amount":5}}}"#;

        let event = priced_event(&platform, superchat);
        assert_eq!(
            event.words,
            Some(50),
            "and a YouTube price must actually reach a Super Chat",
        );
    }

    #[test]
    fn an_unpriced_astro_event_grants_nothing_and_says_so() {
        // The other half: a blank Events box is a decision, and it should be
        // visible rather than a number that quietly stays still.
        let mut settings = Settings::default();
        settings.event_words.insert("twitch:cheer".into(), 1);

        let platform = AstroPlatform::new(Vec::new(), astro_pricing(&settings));

        let follow = r#"{"type":"message","topic":"channel.activities",
          "data":{"type":"follow","provider":"twitch",
            "data":{"displayName":"Someone"}}}"#;

        let event = priced_event(&platform, follow);

        assert_eq!(
            event.words, None,
            "an event nobody priced is worth nothing, and says so rather than being zero",
        );
    }

    #[test]
    fn astro_prices_are_the_same_prices_the_other_platforms_use() {
        // `twitch:cheer` means the same thing arriving through EventSub and
        // through Astro. Sharing the table is what lets a streamer switch source
        // without re-pricing everything, and it is why Astro gets the whole map
        // rather than a filtered one.
        let mut settings = Settings::default();
        settings.event_words.insert("twitch:cheer".into(), 1);
        settings.event_words.insert("chat:command".into(), 100);

        let pricing = astro_pricing(&settings);

        assert_eq!(pricing.get("twitch:cheer"), Some(&1));
        assert_eq!(pricing.get("chat:command"), Some(&100));
        assert_eq!(
            pricing.get("cheer"),
            None,
            "keys must keep their platform prefix, or nothing can look them up",
        );
    }

    /// Price one captured frame, the way the read loop does.
    ///
    /// Goes through the platform's own parser rather than rebuilding the wire
    /// types, so the test crosses the real seam: a JSON payload in, an `Event`
    /// out, priced from the settings file.
    fn priced_event(platform: &AstroPlatform, frame: &str) -> yap_core::goal::Event {
        platform.event_from_json(frame).expect("a priced event")
    }
}

#[cfg(test)]
mod reload_tests {
    use super::*;
    use yap_core::settings::{EngineChoice, LocalBackend, Settings};
    use yap_server::state::CaptureNotice;

    fn local(path: &str) -> EngineChoice {
        EngineChoice::Local {
            model_path: std::path::PathBuf::from(path),
            backend: LocalBackend::default(),
        }
    }

    #[test]
    fn a_model_change_is_a_capture_change() {
        // The whole feature rests on this: a different model has to be something
        // the supervisor notices. If it were not, saving would write the file and
        // the microphone would keep using the old one — which is exactly the
        // behaviour this replaced.
        let before = crate::hub::CaptureConfig::of(&Settings::default());
        let settings = Settings {
            engine: local("/models/other.bin"),
            ..Settings::default()
        };
        assert_ne!(
            before,
            crate::hub::CaptureConfig::of(&settings),
            "swapping the model must be visible to the microphone thread",
        );
    }

    #[test]
    fn a_microphone_change_is_a_capture_change_too() {
        // `input_device` and the VAD are threaded into `Listener::start` beside
        // the engine, so leaving them out of the comparison would mean those
        // settings silently did nothing — the same class of bug as the one this
        // replaced, one level down.
        let before = crate::hub::CaptureConfig::of(&Settings::default());
        let settings = Settings {
            input_device: "Blue Yeti".to_string(),
            ..Settings::default()
        };
        assert_ne!(before, crate::hub::CaptureConfig::of(&settings));
    }

    #[test]
    fn a_colour_change_is_not_a_capture_change() {
        // The other direction, and the reason the comparison is on a named
        // subset rather than "the settings changed". Tearing the microphone down
        // to apply an overlay colour would cost a word of counting mid-stream.
        let mut settings = Settings::default();
        let before = crate::hub::CaptureConfig::of(&settings);

        settings.overlay_theme = "neon".to_string();
        settings.overlay_style = "big".to_string();
        settings.accent = "pink".to_string();

        assert_eq!(
            before,
            crate::hub::CaptureConfig::of(&settings),
            "only capture settings may move the microphone",
        );
    }

    #[test]
    fn a_whitespace_device_is_the_default_device_not_a_new_one() {
        // The device name comes from a text field. Trimming is what makes " "
        // mean "system default" rather than "a microphone called ' '", and the
        // trim has to happen before the comparison or every save of an untouched
        // field would look like a change and rebuild the microphone.
        let settings = Settings {
            input_device: "  ".to_string(),
            ..Settings::default()
        };
        assert_eq!(
            crate::hub::CaptureConfig::of(&settings),
            crate::hub::CaptureConfig::of(&Settings::default()),
        );
    }

    #[test]
    fn saving_reloads_only_when_capture_settings_moved() {
        // End to end through the hub, which is what decides: the notification is
        // sent by the app, not by the panel, so a panel that forgot to ask could
        // not be caught by testing the panel.
        let (mut hub, _messages) = Hub::new(Settings::default());
        let state = hub.state();

        let seen = state
            .watch_capture()
            .expect("the channel exists once watched");
        assert!(!seen.has_changed().unwrap_or(false), "no notice yet");

        // A colour-only save must not wake the microphone.
        let colour = Settings {
            overlay_theme: "neon".to_string(),
            ..Settings::default()
        };
        assert!(!hub.apply_saved_settings(colour), "not a capture change");
        assert!(
            !seen.has_changed().unwrap_or(false),
            "the microphone must not be woken for a colour change",
        );

        // A model change must.
        let model = Settings {
            engine: local("/models/other.bin"),
            ..hub.settings().clone()
        };
        assert!(hub.apply_saved_settings(model), "a model change counts");
        assert!(
            seen.has_changed().unwrap_or(false),
            "the microphone must be woken for a model change",
        );
    }

    #[tokio::test]
    async fn a_notice_that_moves_nothing_does_not_end_the_wait() {
        // The reason the supervisor compares rather than rebuilding on every
        // wakeup. A save that rewrites the settings file without moving the
        // microphone sends a notice like any other; rebuilding then would take
        // the microphone down and cost the streamer a word for nothing.
        let (_tx, mut rx) = tokio::sync::watch::channel(CaptureNotice);
        let settings = Settings::default();
        let running = crate::hub::CaptureConfig::of(&settings);

        let woke = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            wait_for_capture_change(&mut rx, &settings, &running),
        )
        .await;

        assert!(
            woke.is_err(),
            "a notice that did not move the microphone must not end the wait",
        );
    }

    #[tokio::test]
    async fn the_real_channel_carries_a_model_change_to_the_wait() {
        // The two halves above are checked in isolation: the comparison, and the
        // channel. This is them together, through `AppState` — the actual path a
        // save takes, since the panel has a session entity and not a hub.
        //
        // Without it, a notice that never reached the watcher would leave every
        // other test here passing and the microphone loading its model once and
        // never again.
        let state = yap_server::AppState::shared();
        let mut notices = state
            .watch_capture()
            .expect("the channel exists once watched");

        let running = crate::hub::CaptureConfig::of(&Settings::default());

        // Nothing has moved, so the wait must hold even though the channel is
        // live and sendable.
        let quiet = Settings::default();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                wait_for_capture_change(&mut notices, &quiet, &running),
            )
            .await
            .is_err(),
            "the wait must not end before anything changes",
        );

        // What the panel does on save.
        state.notify_capture_changed();

        let moved = Settings {
            engine: local("/models/other.bin"),
            ..Settings::default()
        };
        let fired = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            wait_for_capture_change(&mut notices, &moved, &running),
        )
        .await;

        assert!(
            matches!(fired, Ok(true)),
            "a model change must reach the microphone thread and be acted on",
        );
    }

    #[tokio::test]
    async fn a_notice_that_moves_the_microphone_ends_the_wait() {
        let (_tx, mut rx) = tokio::sync::watch::channel(CaptureNotice);
        let running = crate::hub::CaptureConfig::of(&Settings::default());

        let moved = Settings {
            engine: local("/models/other.bin"),
            ..Settings::default()
        };

        assert!(
            wait_for_capture_change(&mut rx, &moved, &running).await,
            "a model change must end the wait so the microphone can rebuild",
        );
    }
}
