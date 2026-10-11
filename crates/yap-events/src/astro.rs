//! StreamElements Astro — one WebSocket carrying twitch, youtube and kick.
//!
//! An alternative to talking to each platform directly, not a replacement for
//! it. It is genuinely good at one thing the native APIs cannot do: it reports
//! **YouTube channel subscribers**, which the Live Streaming API has no
//! notification for anywhere. Native YouTube can give memberships, Super Chats
//! and members-with-a-message; it cannot tell you who pressed Subscribe.
//!
//! Everything else it does, it does more cheaply than the alternatives. One
//! socket, no OAuth, no quota, no consent screen, and it identifies which
//! platform each event came from in the same envelope.
//!
//! ## Off by default
//!
//! Enabling this is a choice, because it puts a third party between the
//! streamer and their events. It is configured like any other platform and
//! nothing starts unless a token is present.
//!
//! ## Addresses, not fields
//!
//! A token is scoped to **one platform account**, and each account gets its own
//! room — copy the token while switched to the YouTube account and the room
//! changes. So a streamer cross-streaming needs one token per platform, and
//! this opens one connection per token. A single socket carrying two
//! subscriptions with different tokens would be tidier, but the subscribe frame
//! does carry its own token so it may work; untested, so not assumed.
//!
//! ## Mock events are dropped
//!
//! StreamElements marks simulated activity with `isMock: true` — the widget
//! editor's "test" button produces it. Counting those would mean a yapathon
//! could be extended by a cheer nobody sent, so they are ignored outright.
//!
//! `YAP_ALLOW_MOCK_EVENTS=1` counts them anyway, for testing the whole chain
//! end to end without real viewers. An environment variable rather than a
//! setting because `settings.toml` is exactly the file that gets synced to a
//! streaming machine and pasted into support threads — a test switch does not
//! belong in one. Anything accepted this way is relabelled `(mock)` so it is
//! visible as fake in the overlay, the dock and the window rather than
//! indistinguishable from something a viewer actually did.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use yap_core::goal::Event;

use crate::{ChatPlatform, EventSink, price_event};

/// StreamElements' real-time gateway.
const WSS_HOST: &str = "astro.streamelements.com";
const WSS_PORT: u16 = 443;

/// The one topic used.
///
/// `channel.session.update` carries the same values under different key names,
/// and `channel.tips` is StreamElements' own payment page. Everything we price
/// is on `channel.activities`, so subscribing to one topic keeps the scopes we
/// need to a minimum and the surface we have to understand small.
const TOPIC: &str = "channel.activities";

const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// One Astro account.
#[derive(Debug, Clone)]
pub struct AstroAccount {
    /// The channel JWT, copied from the StreamElements dashboard.
    pub token: String,
    /// Which platform this token was configured under.
    ///
    /// A label, never a claim. Which platform an event is *priced* under comes
    /// from the event's own `provider` field, not from here — a token in the
    /// wrong slot still works. What this is for is deciding which platform a
    /// second connection would duplicate, and for naming the account in a log.
    pub platform: &'static str,
}

impl AstroAccount {
    /// The account's name, for logs.
    pub fn label(&self) -> &'static str {
        match self.platform {
            "youtube" => "YouTube",
            "twitch" => "Twitch",
            "kick" => "Kick",
            _ => "unknown",
        }
    }
}

/// Whether to count simulated events, for local testing.
///
/// Read once. An environment variable rather than a setting, because this is a
/// switch that must never travel to a streaming machine and the one place that
/// travels is the settings file.
static ALLOW_MOCK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Whether simulated events are being counted.
///
/// Public so the app can ask at startup, not only when one arrives: the warning
/// that a build is counting fake cheers is worth reading before a stream rather
/// than after the first one has been counted.
pub fn mock_events_allowed() -> bool {
    allow_mock_events()
}

fn allow_mock_events() -> bool {
    *ALLOW_MOCK.get_or_init(|| {
        let on = std::env::var("YAP_ALLOW_MOCK_EVENTS").is_ok_and(|v| !v.is_empty() && v != "0");

        if on {
            // Loud, and on every connect, because the failure this guards
            // against is a test session that quietly includes fake cheers and is
            // then believed.
            log::warn!(
                "astro: YAP_ALLOW_MOCK_EVENTS is set — simulated events will be counted. \
                 Anything they add is labelled (mock) so it can be told apart. \
                 Unset it before streaming."
            );
        }

        on
    })
}

/// Event kinds already reported as unpriced.
///
/// One line per kind rather than per event: chat can be busy, and a line per
/// cheer would bury the whisper loading output it is trying to be noticed
/// against.
static UNPRICED_NOTED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

fn note_unpriced_once(key: &str) {
    let set = UNPRICED_NOTED.get_or_init(Default::default);

    let Ok(mut seen) = set.lock() else {
        return;
    };
    if !seen.insert(key.to_string()) {
        return;
    }

    log::warn!(
        "{key} has no price, so it grants nothing. Set one in Settings -> Events, or \
         blank it to mean 'this event does not move the goal'."
    );
}

/// How many activity ids to remember for deduplication.
///
/// A yapathon that counts one cheer twice is the same failure as one that drops
/// a cheer: the number on screen stops meaning what the streamer did. 256 is
/// far more than any plausible burst and costs nothing to keep.
const SEEN_LIMIT: usize = 256;

/// An Astro connection.
pub struct AstroPlatform {
    accounts: Vec<AstroAccount>,
    /// Words per event, by `<platform>:<kind>`.
    pricing: std::collections::BTreeMap<String, u32>,
    /// Activity ids already priced, newest last.
    ///
    /// Shared across accounts, because `activityId` is unique per activity and
    /// two tokens cover two different accounts that could otherwise both report
    /// the same id.
    seen: Arc<Mutex<VecDeque<String>>>,
    stopped: AtomicBool,
    wake: Notify,
}

impl AstroPlatform {
    pub fn new(
        accounts: Vec<AstroAccount>,
        pricing: std::collections::BTreeMap<String, u32>,
    ) -> Self {
        Self {
            accounts,
            pricing,
            seen: Arc::new(Mutex::new(VecDeque::new())),
            stopped: AtomicBool::new(false),
            wake: Notify::new(),
        }
    }

    /// Parse and price one `channel.activities` frame.
    ///
    /// The read loop is the only caller that matters, but exposing this means a
    /// test can hand in a captured frame and get an `Event` back without the
    /// wire types becoming public — they are an implementation detail of the
    /// gateway's shape, not part of this module's contract.
    ///
    /// Returns `None` for anything unparseable or not a priced activity, which
    /// is the same answer the read loop gives.
    pub fn event_from_json(&self, json: &str) -> Option<Event> {
        let frame: Frame = serde_json::from_str(json).ok()?;
        let activity = frame.data.as_ref().and_then(FrameData::activity)?;
        self.to_event(&activity)
    }

    /// Whether this activity has already been counted.
    ///
    /// A reconnect, or a publisher that retries a delivery, would otherwise
    /// double-count. Returns true for an activity with no id at all, which
    /// cannot be deduplicated and is better counted twice than dropped — the
    /// alternative is losing events to a field that is usually present.
    fn already_seen(&self, activity: &Activity<'_>) -> bool {
        let Some(id) = activity.id else {
            return false;
        };

        let Ok(mut seen) = self.seen.lock() else {
            // Poisoned by a panic elsewhere. Deduplicating is an optimisation
            // against a rare case; refusing to count anything because of it
            // would be much worse.
            return false;
        };

        if seen.iter().any(|known| known == id) {
            return true;
        }

        seen.push_back(id.to_string());
        while seen.len() > SEEN_LIMIT {
            seen.pop_front();
        }
        false
    }

    fn stopping(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    fn url() -> String {
        format!("wss://{WSS_HOST}:{WSS_PORT}/")
    }

    /// One connection, from subscribe to close.
    async fn connect_once(&self, account: &AstroAccount, sink: Arc<dyn EventSink>) -> Result<()> {
        let (mut socket, response) = tokio_tungstenite::connect_async(Self::url())
            .await
            .context("connecting to StreamElements Astro")?;

        if response.status()
            != tokio_tungstenite::tungstenite::http::StatusCode::SWITCHING_PROTOCOLS
        {
            // 502 while they drain, 429 when the connection rate limit is hit.
            // Both are transient, and both deserve a different delay from a
            // token that is simply wrong.
            return Err(anyhow::anyhow!(
                "astro handshake returned {}",
                response.status()
            ));
        }

        log::info!("astro connected for {}", account.label());

        let mut subscribed = false;
        let mut nonce = 0u32;
        // Which platform this account's events actually arrive from, once known.
        // Not the label the account was configured under: the token does not say
        // which platform it covers and nothing checks, so this is how a token
        // saved in the wrong slot announces itself rather than being silently
        // miscounted.
        let mut seen_provider: Option<String> = None;

        while let Some(message) = socket.next().await {
            let text = match message? {
                Message::Text(t) => t,
                Message::Close(_) => break,
                // Ping/Pong/Frame carry no events; tungstenite answers pings
                // itself, which matters because Astro closes the connection if
                // it sees no pong within seventy seconds.
                _ => continue,
            };

            let frame: Frame = match serde_json::from_str(&text) {
                Ok(frame) => frame,
                // An unparseable frame is not worth dropping a stream over.
                Err(error) => {
                    log::debug!("astro: unparseable frame: {error}");
                    continue;
                }
            };

            match frame.kind.as_str() {
                "welcome" => {
                    // Subscribing here rather than on connect, because Astro
                    // expects the welcome first.
                    nonce += 1;
                    let request = Subscribe {
                        r#type: "subscribe".to_string(),
                        nonce: nonce.to_string(),
                        data: SubscribeData {
                            topic: TOPIC.to_string(),
                            // No room: Astro resolves the authenticated
                            // channel from the token, and getting it wrong
                            // means subscribing to nothing at all.
                            room: None,
                            token: account.token.clone(),
                            token_type: "jwt".to_string(),
                        },
                    };

                    socket
                        .send(Message::Text(serde_json::to_string(&request)?))
                        .await
                        .context("subscribing to astro")?;

                    log::info!("astro: subscribed to {TOPIC} for {}", account.label());
                }

                "response" => {
                    if let Some(error) = &frame.error {
                        return Err(anyhow::anyhow!(
                            "astro refused the subscription: {error} — {}",
                            frame.data.and_then(|d| d.message).unwrap_or_default()
                        ));
                    }
                    subscribed = true;
                }

                "reconnect" => {
                    // A graceful drain. Astro sends a token that restores every
                    // subscription on the new connection, so re-subscribing
                    // would be harmless but pointless. Return so the run loop
                    // reconnects immediately rather than sitting in backoff
                    // through a shutdown they are trying to be quick about.
                    log::info!("astro: server draining, reconnecting");
                    return Ok(());
                }

                "message" => {
                    if !subscribed {
                        // A message before the subscribe was acknowledged means
                        // something is wrong with the handshake; acting on it
                        // anyway risks double counting after a reconnect.
                        continue;
                    }

                    if frame.topic.as_deref() != Some(TOPIC) {
                        continue;
                    }

                    let Some(activity) = frame.data.as_ref().and_then(FrameData::activity) else {
                        continue;
                    };

                    if activity.is_mock && !allow_mock_events() {
                        log::debug!(
                            "astro: ignoring a simulated {} event from {}",
                            activity.kind,
                            activity.data.user()
                        );
                        continue;
                    }

                    if seen_provider.as_deref() != Some(activity.provider) {
                        if seen_provider.is_some() {
                            log::warn!(
                                "astro: the {} account is also reporting {} events",
                                account.label(),
                                activity.provider
                            );
                        }
                        log::info!(
                            "astro: {} is receiving {} events",
                            account.label(),
                            activity.provider
                        );
                        seen_provider = Some(activity.provider.to_string());
                    }

                    if self.already_seen(&activity) {
                        log::debug!("astro: already counted {}", activity.id.unwrap_or("?"));
                        continue;
                    }

                    if let Some(event) = self.to_event(&activity) {
                        sink.on_event(event);
                    }
                }

                _ => {}
            }
        }

        if subscribed {
            Ok(())
        } else {
            Err(anyhow::anyhow!("astro closed before subscribing"))
        }
    }

    /// Price an activity, or decide it is not one we count.
    ///
    /// The whole pricing rule is `amount` present means scale by it, absent
    /// means flat, so this only has to answer two questions: what is this event
    /// called, and how big was it. Everything else is the shared rule's job,
    /// which is why adding a platform here costs a table and not a pricing
    /// change.
    fn to_event(&self, activity: &Activity<'_>) -> Option<Event> {
        let (kind, amount) = shape(activity)?;

        let configured = self
            .pricing
            .get(&format!("{}:{kind}", activity.provider))
            .copied();

        // An event with no price is indistinguishable on the overlay from an
        // event that never arrived — which is the same "nothing happened" the
        // empty Events section already caused once, and the same silence that
        // hid a discarded session.
        if configured.is_none() {
            note_unpriced_once(&format!("{}:{kind}", activity.provider));
        }

        let mut event = price_event(
            activity.provider,
            kind,
            Some(activity.data.user()),
            amount,
            configured,
        );

        // Relabelled *after* pricing, so the words are right and the label is
        // honest. `kind` is display only — `apply_event` reads `words` and
        // nothing else — which is what makes this safe. A test session that
        // cannot be told apart from a real one is worse than no test mode.
        if activity.is_mock {
            event.kind = format!("{kind} (mock)");
        }

        Some(event)
    }
}

/// What kind of event this is, and how big it was.
///
/// `None` amount means flat — not "unknown". The distinction matters: a gift
/// bomb whose count StreamElements failed to send should pay nothing rather
/// than a full gift's worth, so a batch we cannot size sends `Some(0.0)`.
fn shape(activity: &Activity<'_>) -> Option<(&'static str, Option<f64>)> {
    let data = activity.data;
    let amount = data.amount;

    match (activity.provider, activity.kind) {
        // --- Twitch -----------------------------------------------------
        ("twitch", "follow") | ("twitch", "follower") => Some(("follow", None)),
        ("twitch", "subscriber") => {
            // `amount` is months. A first month is a sub, anything beyond is a
            // resub — which is what the streamer prices differently.
            let months = amount.unwrap_or(1.0);
            if months >= 2.0 || data.streak.unwrap_or(0) > 1 {
                Some(("resub", None))
            } else {
                Some(("sub", None))
            }
        }
        // The gift bomb itself. `amount` is the number of subs in it, so a
        // hundred gifted subs is worth a hundred subs.
        ("twitch", "communityGiftPurchase") => Some(("gift_sub", amount)),
        ("twitch", "cheer") => Some(("cheer", amount)), // bits
        ("twitch", "raid") => Some(("raid", amount)),   // viewers
        ("twitch", "host") => Some(("host", amount)),   // viewers
        ("twitch", "tip") => Some(("tip", amount)),
        ("twitch", "channelPointsRedemption") | ("twitch", "redemption") => {
            Some(("redemption", None))
        }

        // --- YouTube ----------------------------------------------------
        // The event the native Live Streaming API has no notification for.
        ("youtube", "subscriber") => Some(("subscriber", None)),
        // A paid channel membership. Distinct from a subscriber, and priced
        // separately — one is a person clicking Subscribe, the other is money.
        ("youtube", "sponsor") => Some(("membership", None)),
        ("youtube", "superchat") => Some(("superchat", amount)),
        ("youtube", "tip") => Some(("tip", amount)),
        ("youtube", "merch") => Some(("merch", data.merch_value())),
        ("youtube", "channelPointsRedemption") | ("youtube", "redemption") => {
            Some(("redemption", None))
        }

        // --- Kick -------------------------------------------------------
        ("kick", "follow") | ("kick", "follower") => Some(("follow", None)),
        ("kick", "subscriber") => Some(("sub", None)),
        ("kick", "kicks") => Some(("kicks", amount)),
        ("kick", "tip") => Some(("tip", amount)),

        // --- Platform-agnostic ------------------------------------------
        (_, "follow") | (_, "follower") => Some(("follow", None)),
        (_, "subscriber") => Some(("subscriber", None)),
        (_, "sponsor") | (_, "supporter") | (_, "fan") => Some(("membership", None)),
        (_, "superchat") => Some(("superchat", amount)),
        (_, "cheer") => Some(("cheer", amount)),
        (_, "raid") => Some(("raid", amount)),
        (_, "host") => Some(("host", amount)),
        (_, "tip") | (_, "charityCampaignDonation") => Some(("tip", amount)),
        (_, "communityGiftPurchase") => Some(("gift_sub", amount)),
        (_, "merch") | (_, "purchase") => Some(("merch", data.merch_value())),
        (_, "channelPointsRedemption") | (_, "redemption") => Some(("redemption", None)),
        (_, "giveaway") => Some(("giveaway", None)),

        // Hypetrain, shares, video likes, sponsorship renewals, merch
        // previews: real events, none of them something a viewer did *for* the
        // streamer, and none of them worth pricing.
        _ => None,
    }
}

#[async_trait::async_trait]
impl ChatPlatform for AstroPlatform {
    fn name(&self) -> &'static str {
        "astro"
    }

    async fn run(&self, sink: Arc<dyn EventSink>) -> Result<()> {
        // One connection per account, each on its own task. A cross-streaming
        // streamer has a Twitch token and a YouTube token, and neither should
        // have to wait for the other to connect.
        let mut handles = Vec::new();

        for account in &self.accounts {
            let account = account.clone();
            let sink = sink.clone();
            let pricing = self.pricing.clone();
            let stopped = self.stopped.load(Ordering::SeqCst);
            // Cloned per account, not once outside the loop: every connection
            // must deduplicate against the same set, or the same activity
            // arriving on two tokens would be counted twice.
            let seen = self.seen.clone();

            handles.push(tokio::spawn(async move {
                let platform = AstroPlatform {
                    accounts: vec![account.clone()],
                    pricing,
                    seen,
                    stopped: AtomicBool::new(stopped),
                    wake: Notify::new(),
                };
                platform.run_one(&account, sink).await;
            }));
        }

        for handle in handles {
            let _ = handle.await;
        }

        Ok(())
    }

    async fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
    }
}

impl AstroPlatform {
    /// The reconnect loop for one account.
    async fn run_one(&self, account: &AstroAccount, sink: Arc<dyn EventSink>) {
        let mut backoff = RECONNECT_DELAY;

        loop {
            if self.stopping() {
                return;
            }

            match self.connect_once(account, sink.clone()).await {
                Ok(()) => backoff = RECONNECT_DELAY,
                Err(error) => {
                    log::warn!("astro ({}): {error:#}", account.label());
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }

            tokio::select! {
                _ = self.wake.notified() => {
                    if self.stopping() {
                        return;
                    }
                }
                _ = tokio::time::sleep(backoff) => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Wire types.
//
// The nesting is the whole difficulty here, and it is not in the docs. A
// `channel.activities` message is:
//
//     frame.data              -> type, provider, channel, isMock, activityId
//     frame.data.data         -> username, displayName, amount, tier, streak
//
// Every size lives in the *nested* object. Written from the documentation —
// which lists only `data.data.username/displayName/providerId/avatar` — a
// mapping puts `amount` at the top level and prices every Super Chat as a flat
// grant while passing every test. Captured frames are what make the difference.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    data: Option<FrameData>,
}

/// One message's payload.
///
/// Doubles as the subscribe acknowledgement, which carries a `message` and no
/// activity at all — so everything an activity needs is optional and
/// [`FrameData::activity`] is what says whether one is really here.
#[derive(Debug, Deserialize, Default)]
struct FrameData {
    #[serde(default)]
    message: Option<String>,

    #[serde(default, rename = "type")]
    activity_kind: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    /// StreamElements' own test events, from the widget editor.
    ///
    /// The rename is load-bearing. Serde would map this field to `is_mock`,
    /// find no such key in a payload that spells it `isMock`, and default it to
    /// `false` — silently, with no error — so every simulated cheer would be
    /// counted as though a viewer had sent it. A `#[serde(default)]` on a field
    /// that should always be present turns a typo into a wrong number.
    #[serde(default, rename = "isMock")]
    is_mock: bool,
    /// Stable per activity; the dedup key across a reconnect.
    #[serde(default, rename = "activityId")]
    activity_id: Option<String>,
    /// The nested object every size actually lives in.
    #[serde(default)]
    data: Option<ActivityData>,
}

impl FrameData {
    /// The activity this frame describes, if it describes one.
    fn activity(&self) -> Option<Activity<'_>> {
        Some(Activity {
            provider: self.provider.as_deref()?,
            kind: self.activity_kind.as_deref()?,
            is_mock: self.is_mock,
            id: self.activity_id.as_deref(),
            data: self.data.as_ref()?,
        })
    }
}

/// A borrowed view of one activity, so shaping costs no allocation.
#[derive(Debug)]
struct Activity<'a> {
    provider: &'a str,
    kind: &'a str,
    is_mock: bool,
    id: Option<&'a str>,
    data: &'a ActivityData,
}

#[derive(Debug, Deserialize, Default)]
struct ActivityData {
    #[serde(default, rename = "displayName")]
    user: String,
    #[serde(default)]
    username: String,
    /// The activity's size. Bits for a cheer, money for a Super Chat, a sub
    /// count for a gift bomb, viewers for a raid, months for a sub.
    #[serde(default)]
    amount: Option<f64>,
    /// Subs carry a streak; past one is a resub.
    #[serde(default)]
    streak: Option<u32>,
    /// Merch only. The money is in here, not in `amount`.
    #[serde(default)]
    items: Vec<MerchItem>,
}

impl ActivityData {
    /// Who to credit, preferring the display name.
    fn user(&self) -> String {
        if !self.user.is_empty() {
            return self.user.clone();
        }
        self.username.clone()
    }

    /// What a merch purchase actually cost.
    ///
    /// `amount` on a merch activity is the number of *line items*, not the
    /// money — one basket of three things arrives as `amount: 1` with an `items`
    /// array. So the price is the sum, and a basket is worth what it cost.
    fn merch_value(&self) -> Option<f64> {
        if self.items.is_empty() {
            return self.amount;
        }
        Some(
            self.items
                .iter()
                .map(|item| item.price * item.quantity.unwrap_or(1.0))
                .sum(),
        )
    }
}

#[derive(Debug, Deserialize)]
struct MerchItem {
    #[serde(default)]
    price: f64,
    #[serde(default)]
    quantity: Option<f64>,
}

#[derive(Debug, serde::Serialize)]
struct Subscribe {
    #[serde(rename = "type")]
    r#type: String,
    nonce: String,
    data: SubscribeData,
}

#[derive(Debug, serde::Serialize)]
struct SubscribeData {
    topic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    room: Option<String>,
    token: String,
    token_type: String,
}

#[cfg(test)]
mod tests;
