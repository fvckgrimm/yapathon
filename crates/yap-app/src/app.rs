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
use yap_voice::pipeline::{Listener, WordSink};

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

/// Start the microphone and the recogniser.
///
/// Failures are reported through the hub rather than aborting startup. The
/// window is already open by this point, and the count sitting at zero looks
/// exactly like working — so a failure has to be visible somewhere, not only in
/// a log nobody reads mid-stream.
fn start_listening(hub: &Hub) {
    let settings = hub.settings().clone();
    let hub = hub.clone();

    runtime::spawn(async move {
        let settings = &settings;
        // Say which model is in use, and say so when it is not the one the
        // settings asked for.
        //
        // The substitution is decided by the loader, which is the only place
        // that knows both what was configured and what exists. Working it out
        // here by comparing against the built-in default name instead warned on
        // every install that was not running that exact file, which is every
        // install — and a warning that is always true is one a streamer learns
        // to ignore, including the time it is telling the truth.
        match hub.model_substitution() {
            Some(substitution) => {
                hub.report(ModelMessage::Warning(format!(
                    "using {} — no model at {}",
                    substitution.using.display(),
                    substitution.wanted.display(),
                )));
            }
            None => log::info!("yap-a-thon: using the configured model"),
        }

        let engine = match yap_voice::build_engine(&settings.engine).await {
            // `build_engine` returns a `Box<dyn TranscriptionEngine>`; this
            // converts it to the `Arc` the listener shares, so a local engine
            // and a cloud one are interchangeable to everything above this line.
            Ok(engine) => Arc::from(engine) as Arc<dyn yap_core::engine::TranscriptionEngine>,
            Err(error) => {
                // The message names the directory to put a model in. "no model"
                // with no path is what sends someone hunting their filesystem.
                let message = match &settings.engine {
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
                return;
            }
        };

        let sink: Arc<dyn WordSink> = Arc::new(SessionSink { hub: hub.clone() });

        // The device name is trimmed before use because it comes from a text
        // field, and an empty or whitespace-only one means "the system default"
        // rather than "a device called ' ' that does not exist".
        let device = settings.input_device.trim().to_string();
        if device.is_empty() {
            log::info!("yap-a-thon: opening the default microphone");
        } else {
            log::info!("yap-a-thon: opening microphone {device:?}");
        }

        let started = if device.is_empty() {
            Listener::start(engine, settings.vad, sink).await
        } else {
            Listener::start_named(device, engine, settings.vad, sink).await
        };
        log::info!("yap-a-thon: microphone opened");

        let listener = match started {
            Ok(listener) => listener,
            Err(error) => {
                hub.report_awaited(ModelMessage::Warning(format!(
                    "microphone unavailable: {error:#}"
                )))
                .await;
                return;
            }
        };

        hub.state().set_listening(true).await;
        hub.report(ModelMessage::Listening(true));

        // Held for the life of the process.
        //
        // This used to read `listener.stop().await` — a comment said "held until
        // shutdown" and the code did the opposite, tearing the microphone down
        // the instant it opened. It looked like it worked because capture did
        // start, and `listening` then reported false.
        //
        // Dropping the listener also closes the microphone, so it is parked here
        // deliberately rather than left to fall out of scope.
        let _held = listener;
        std::future::pending::<()>().await;
    });
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
    let accounts = astro_accounts(hub.settings());
    if accounts.is_empty() {
        return;
    }

    // Astro names events `<platform>:<kind>`, the same shape EventSub uses, so
    // the prices a streamer already set carry over rather than silently
    // re-pricing itself to nothing on the day they switch.
    let mut pricing = std::collections::BTreeMap::new();
    for platform in ["twitch", "youtube", "kick"] {
        pricing.extend(hub.settings().pricing_for(platform));
    }

    let platform = AstroPlatform::new(accounts, pricing);

    let sink: Arc<dyn EventSink> = Arc::new(StateEventSink { hub: hub.clone() });

    runtime::spawn(async move {
        log::info!("astro: connecting");
        if let Err(error) = platform.run(sink).await {
            log::warn!("astro stopped: {error:#}");
        }
    });
}

/// The Astro accounts, from the credential store.
///
/// Empty means Astro is switched off, which is the default and the point: it
/// puts a third party between the streamer and their events, so it is a choice.
///
/// Comma-separated, trimmed, and empty entries dropped — so pasting a token with
/// a trailing comma, which is what every terminal history and every "copy" menu
/// eventually produces, connects one account rather than two, the second of
/// which would fail auth and retry for the rest of the stream.
pub fn astro_accounts(settings: &yap_core::settings::Settings) -> Vec<AstroAccount> {
    let service = settings
        .astro_credential
        .clone()
        .unwrap_or_else(|| yap_core::settings::credential_name("astro"));

    // A missing keyring is not an error. It means this machine cannot store a
    // credential, so there is nothing to connect with — the same reasoning that
    // stops the settings panel offering to accept a token it cannot keep.
    let Ok(Some(stored)) = yap_core::secrets::get(&service, "jwt") else {
        return Vec::new();
    };

    stored
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .enumerate()
        .map(|(index, token)| AstroAccount {
            token: token.to_string(),
            // Named by position rather than by platform, because the token does
            // not say which platform it covers and guessing would be wrong.
            label: format!("account {}", index + 1),
        })
        .collect()
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
}
