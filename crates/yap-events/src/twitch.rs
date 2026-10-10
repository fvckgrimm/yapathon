//! Twitch EventSub over WebSocket.
//!
//! The cleanest of the three platforms: one socket carries subs, resubs, gift
//! subs, bits, raids and redemption redemptions, authenticated with a user
//! token and needing no inbound endpoint. That last part matters for an app on
//! a streamer's machine behind NAT — nothing has to be reachable from the
//! internet.
//!
//! The socket drops. WebSockets do, chat goes quiet, Twitch cycles connections.
//! Reconnection is handled here rather than in the core, because a yapathon
//! that silently stops counting subs is one whose goal stops meaning anything.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::http::StatusCode;
use yap_core::goal::Event;

use crate::{ChatPlatform, EventSink, price_event};

/// Twitch's EventSub WebSocket host.
///
/// The application id goes in the query string: EventSub rejects a connection
/// without one before it even looks at the bearer token.
const WSS_HOST: &str = "eventsub.wss.twitch.tv";
const WSS_PORT: u16 = 443;
const WSS_PATH: &str = "/ws";

/// Open the EventSub socket for a registered application.
async fn connect_with_client_id(
    client_id: &str,
) -> Result<(
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::handshake::client::Response,
)> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::header::HeaderName;

    // `keepalive_timeout_seconds` is raised because the default is 10 minutes
    // and Twitch closes the socket at that point. A stream is often longer, and a
    // reconnect mid-yapathon is a gap where subs stop counting.
    let mut request =
        format!("wss://{WSS_HOST}:{WSS_PORT}{WSS_PATH}?keepalive_timeout_seconds=600")
            .into_client_request()
            .map_err(|e| anyhow::anyhow!("building the eventsub request: {e}"))?;

    let header = HeaderName::from_static("client-id");
    let value = client_id.parse().map_err(|_| {
        anyhow::anyhow!("the twitch client id contains characters a header value cannot")
    })?;
    request.headers_mut().insert(header, value);

    tokio_tungstenite::connect_async(request)
        .await
        .map_err(Into::into)
}

/// The credential-store user for the Twitch token.
const KEYRING_USER: &str = "user-token";

/// How long to wait before reconnecting after a drop.
///
/// Twitch's own guidance is to reconnect promptly rather than back off far, but
/// not instantly: a hard-down endpoint would otherwise turn this into a busy
/// loop that hammers Twitch from every streamer who left it running.
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// Maximum backoff between reconnect attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Event kinds worth listening for.
///
/// Deliberately not "everything". An event kind that arrives and grants nothing
/// costs a socket message per occurrence and does nothing else.
pub const SUBSCRIPTION_TYPES: &[&str] = &[
    "channel.subscribe",
    "channel.subscription.message",
    "channel.subscription.gift",
    "channel.cheer",
    "channel.raid",
    "channel.channel_points_custom_reward_redemption.add",
];

/// The kind string used in settings and in the event feed.
///
/// EventSub type names are namespaced (`channel.subscription.gift`); the goal
/// settings read better short, so the trailing component is used.
fn short_kind(event_type: &str) -> &str {
    match event_type {
        "channel.subscribe" => "sub",
        "channel.subscription.message" => "resub",
        "channel.subscription.gift" => "gift_sub",
        "channel.cheer" => "cheer",
        "channel.raid" => "raid",
        "channel.channel_points_custom_reward_redemption.add" => "redemption",
        // An unrecognised type still arrives; giving it the full name keeps the
        // feed honest rather than inventing a category.
        other => other,
    }
}

/// How a platform-native amount maps onto words.
///
/// Kept separate from the wire types so the pricing logic is testable without
/// a socket.
#[derive(Debug, Clone)]
pub struct TwitchPricing {
    /// Words per event, by short kind.
    pub words: std::collections::BTreeMap<String, u32>,
}

impl TwitchPricing {
    /// Prices from a `<platform>:<kind>` map, as the settings file stores it.
    ///
    /// Unprefixed kinds in, because a platform's pricing is its own concern.
    pub fn from_words(words: std::collections::BTreeMap<String, u32>) -> Self {
        Self { words }
    }

    pub fn get(&self, kind: &str) -> Option<u32> {
        self.words.get(kind).copied()
    }
}

impl Default for TwitchPricing {
    fn default() -> Self {
        let mut words = std::collections::BTreeMap::new();
        words.insert("sub".to_string(), 100);
        words.insert("resub".to_string(), 100);
        words.insert("gift_sub".to_string(), 50);
        // Words per bit. Small on purpose: 100 bits is worth a dollar, and one
        // word per bit makes a 10000-bit cheer add 10000 words, which would end
        // any run.
        words.insert("cheer".to_string(), 1);
        words.insert("raid".to_string(), 250);
        words.insert("redemption".to_string(), 100);
        Self { words }
    }
}

/// An EventSub notification.
#[derive(Debug, Deserialize)]
pub struct Notification {
    pub metadata: Metadata,
    pub payload: Payload,
}

#[derive(Debug, Deserialize)]
pub struct Metadata {
    pub message_type: String,
    #[serde(default)]
    pub message_id: Option<String>,
    /// Present on the session welcome, which is the first frame on a new socket.
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Payload {
    pub subscription: Subscription,
}

#[derive(Debug, Deserialize)]
pub struct Subscription {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub user_id: Option<String>,
    /// The event body, when the notification carries one.
    ///
    /// Twitch's websocket notifications inline the concrete event, so this is
    /// where the sizes actually are. It was not being read at all, which is why
    /// a hundred gifted subs and one gifted sub granted the same handful of
    /// words: the pricing code scales by amount, and the amount was always
    /// missing.
    #[serde(default)]
    pub event: Option<EventBody>,
}

/// The inline event body of a notification.
#[derive(Debug, Deserialize)]
pub struct EventBody {
    /// Bits cheered, on `channel.cheer`.
    #[serde(default)]
    pub bits: Option<u64>,
    /// Subs gifted in this batch, on `channel.subscription.gift`.
    #[serde(default)]
    pub cumulative_total: Option<u64>,
    /// Viewers brought by a raid.
    #[serde(default)]
    pub viewers: Option<u64>,
}

#[cfg(test)]
impl EventBody {
    /// A body with nothing in it, for kinds that carry no size.
    fn empty() -> Self {
        Self {
            bits: None,
            cumulative_total: None,
            viewers: None,
        }
    }
}

/// Convert an EventSub notification into a priced event.
///
/// # Errors
/// Returns a message when the notification is not an event we price, which is
/// ordinary — keepalives and subscription confirmations share this shape.
pub fn notification_to_event(
    notification: &Notification,
    pricing: &TwitchPricing,
) -> Result<Event, &'static str> {
    if notification.metadata.message_type != "notification" {
        return Err("not an event notification");
    }

    // The subscription *type* identifies the event; the inline body carries its
    // size. Both are needed: pricing scales by amount, and a hundred gifted subs
    // priced the same as one is exactly the kind of arithmetic a streamer cannot
    // correct afterwards.
    let subscription = &notification.payload.subscription;
    let kind = short_kind(&subscription.kind);
    let amount = amount_for(kind, subscription.event.as_ref());

    Ok(price_event("twitch", kind, None, amount, pricing.get(kind)))
}

/// The amount this event should be priced by.
///
/// `None` for kinds that are one event whatever they contain, so the configured
/// value is a flat grant. `Some(0.0)` for a kind whose size we need and failed
/// to read — a gift bomb with no body is not "one sub", and paying the single-sub
/// price for an unknown number of them is how a notification we failed to parse
/// turns into free words.
///
/// Twitch does not carry a viewer count on the raid notification we handle, so a
/// raid is flat. Pricing raids per raider is a real choice streamers make, so it
/// belongs in the settings and the body rather than being decided here.
fn amount_for(kind: &str, body: Option<&EventBody>) -> Option<f64> {
    match kind {
        "cheer" => Some(body.and_then(|body| body.bits).unwrap_or(0) as f64),
        "gift_sub" => Some(body.and_then(|body| body.cumulative_total).unwrap_or(0) as f64),
        _ => None,
    }
}

/// A live Twitch connection.
pub struct TwitchPlatform {
    /// The channel to subscribe to.
    channel: String,
    /// User access token with the channel scopes.
    ///
    /// Used when the subscription handshake runs; until then it is only carried,
    /// which is why the field is read through the accessor rather than directly.
    token: String,
    /// The registered Twitch application id.
    ///
    /// Public, and required by EventSub on the WebSocket URL itself. Not a
    /// secret; the secret never appears in this crate.
    client_id: String,
    pricing: TwitchPricing,
    /// Set by [`ChatPlatform::stop`]; checked at the top of every reconnect.
    stopped: Arc<AtomicBool>,
    /// Woken by `stop` so a sleeping loop returns at once.
    wake: Arc<Notify>,
}

impl TwitchPlatform {
    pub fn new(channel: String, token: String, client_id: String, pricing: TwitchPricing) -> Self {
        Self {
            channel,
            token,
            client_id,
            pricing,
            stopped: Arc::new(AtomicBool::new(false)),
            wake: Arc::new(Notify::new()),
        }
    }

    /// Load the user token from the OS credential store.
    ///
    /// # Errors
    /// Names the service when nothing is stored, because "no token" is the most
    /// common reason this platform does not work and it should not present as a
    /// network failure.
    pub fn from_keyring(
        channel: String,
        service: &str,
        client_id: &str,
        pricing: TwitchPricing,
    ) -> Result<Self> {
        let entry =
            keyring::Entry::new(service, KEYRING_USER).context("credential store unavailable")?;
        let token = entry
            .get_password()
            .with_context(|| format!("no stored Twitch token for {service}"))?;

        Ok(Self::new(channel, token, client_id.to_string(), pricing))
    }

    /// Store a user token for later use.
    pub fn store_token(service: &str, token: &str) -> Result<()> {
        let entry = keyring::Entry::new(service, KEYRING_USER).context("credential store")?;
        entry
            .set_password(token)
            .context("storing the Twitch token")
    }

    /// The event kinds this connection subscribes to.
    pub fn subscription_types(&self) -> &[&str] {
        SUBSCRIPTION_TYPES
    }

    pub fn pricing(&self) -> &TwitchPricing {
        &self.pricing
    }

    /// The channel this connection watches.
    pub fn channel(&self) -> &str {
        &self.channel
    }

    /// The registered application id.
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Whether a token is present.
    ///
    /// A bool rather than the token: the handshake needs the value, but nothing
    /// outside this type should be able to read a credential.
    pub fn has_token(&self) -> bool {
        !self.token.is_empty()
    }

    /// The subscribe frames for this channel.
    ///
    /// EventSub over WebSocket takes the event types in the message rather than
    /// in a URL, and every subscription carries the same session id, auth token
    /// and broadcaster id. Split out from the socket loop so the request shape is
    /// testable without a connection.
    pub fn subscribe_request(&self, session_id: &str, broadcaster_id: &str) -> SubscribeRequest {
        SubscribeRequest {
            metadata: SubscribeMetadata {
                message_type: "session_keepalive".to_string(),
                message_id: uuid::Uuid::new_v4().to_string(),
                session_id: session_id.to_string(),
            },
            payload: SubscribePayload {
                session_id: session_id.to_string(),
                token: self.token.clone(),
                subscriptions: SUBSCRIPTION_TYPES
                    .iter()
                    .map(|kind| SubscribeItem {
                        kind: (*kind).to_string(),
                        version: EVENT_SUB_VERSION.to_string(),
                        broadcaster_user_id: broadcaster_id.to_string(),
                    })
                    .collect(),
            },
        }
    }

    /// Resolve a channel login to its numeric broadcaster id.
    ///
    /// EventSub keys subscriptions on the id, not the login name, so the login
    /// has to go through Helix first. `client_id` is the registered application
    /// id; Helix rejects the request without one.
    pub async fn resolve_channel_id(
        &self,
        client: &reqwest::Client,
        client_id: &str,
    ) -> Result<String> {
        let response = client
            .get("https://api.twitch.tv/helix/users")
            .query(&[("login", self.channel.as_str())])
            .header("Client-Id", client_id)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("looking up the twitch channel")?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "twitch channel lookup returned {}",
                response.status()
            ));
        }

        let body: UsersResponse = response
            .json()
            .await
            .context("unreadable channel lookup response")?;

        body.data
            .first()
            .map(|user| user.id.clone())
            .with_context(|| format!("no twitch channel named {:?}", self.channel))
    }
}

/// The EventSub websocket protocol version this client speaks.
pub const EVENT_SUB_VERSION: &str = "1";

/// The public application id EventSub requires on every WebSocket connection.
///
/// Twitch rejects a connection without one *before* it looks at the bearer
/// token, so this is required, not optional. It is a public identifier — the
/// secret never appears in this crate. Each streamer registers their own
/// application and puts the id in `settings.toml`; this constant is only the
/// placeholder the setup docs point at.
pub const TWITCH_CLIENT_ID_PLACEHOLDER: &str = "replace-with-your-twitch-client-id";

/// The subscribe frame sent once a session id is known.
#[derive(Debug, serde::Serialize)]
pub struct SubscribeRequest {
    pub metadata: SubscribeMetadata,
    pub payload: SubscribePayload,
}

#[derive(Debug, serde::Serialize)]
pub struct SubscribeMetadata {
    #[serde(rename = "message_type")]
    pub message_type: String,
    #[serde(rename = "message_id")]
    pub message_id: String,
    #[serde(rename = "session_id")]
    pub session_id: String,
}

#[derive(Debug, serde::Serialize)]
pub struct SubscribePayload {
    #[serde(rename = "session_id")]
    pub session_id: String,
    pub token: String,
    pub subscriptions: Vec<SubscribeItem>,
}

#[derive(Debug, serde::Serialize)]
pub struct SubscribeItem {
    #[serde(rename = "type")]
    pub kind: String,
    pub version: String,
    #[serde(rename = "broadcaster_user_id")]
    pub broadcaster_user_id: String,
}

/// Helix's channel lookup response.
#[derive(Debug, serde::Deserialize)]
struct UsersResponse {
    #[serde(default)]
    data: Vec<User>,
}

#[derive(Debug, serde::Deserialize)]
struct User {
    id: String,
}

#[async_trait::async_trait]
impl ChatPlatform for TwitchPlatform {
    fn name(&self) -> &'static str {
        "twitch"
    }

    async fn run(&self, sink: Arc<dyn EventSink>) -> Result<()> {
        let mut backoff = RECONNECT_DELAY;

        loop {
            // Check before connecting, so a stop during backoff is honoured
            // without waiting out the delay first.
            if self.stopping() {
                return Ok(());
            }

            match self.connect_once(sink.clone(), &self.client_id).await {
                Ok(()) => {
                    // A clean close still means reconnect: the socket does not
                    // come back on its own.
                    backoff = RECONNECT_DELAY;
                }
                Err(e) => {
                    log::warn!("twitch eventsub: {e:#}");
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }

            // Wake on stop rather than always sleeping the full delay, so
            // quitting does not wait out a sixty-second backoff.
            tokio::select! {
                _ = self.wake.notified() => {
                    if self.stopping() {
                        return Ok(());
                    }
                }
                _ = tokio::time::sleep(backoff) => {}
            }
        }
    }

    async fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        // `notify_one` stores a permit when nothing is waiting yet, so a stop
        // issued before the loop reaches its `select` is not lost. The flag
        // covers the window regardless; this only makes the wake prompt.
        self.wake.notify_one();
    }
}

impl TwitchPlatform {
    /// Whether a stop has been requested.
    fn stopping(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

impl TwitchPlatform {
    /// One connection's lifetime. Returns on close, so `run` can reconnect.
    async fn connect_once(&self, sink: Arc<dyn EventSink>, client_id: &str) -> Result<()> {
        // Resolved before the socket opens: a subscription cannot be built
        // without it, and failing here gives a clear message instead of a socket
        // that opens and then silently receives nothing.
        let broadcaster_id = self
            .resolve_channel_id(&reqwest::Client::new(), client_id)
            .await?;

        let (mut socket, response) = connect_with_client_id(client_id)
            .await
            .context("connecting to EventSub")?;

        // Twitch answers with `401 Unauthorized` before the socket opens when the
        // token is missing or lacks the channel scopes. Reading the status here
        // turns that into one clear message instead of a reconnect loop that
        // logs a bare "handshake failed" every few seconds for the whole stream.
        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            if response.status() == StatusCode::UNAUTHORIZED {
                return Err(anyhow::anyhow!(
                    "twitch rejected the token: it needs the channel:read:subscriptions scope \
                     and must not be expired"
                ));
            }
            return Err(anyhow::anyhow!(
                "eventsub handshake returned {}",
                response.status()
            ));
        }

        log::info!("twitch eventsub connected for {}", self.channel);

        // Subscribing needs the token, which is why it is read here rather than
        // at construction: nothing else needs it, and a credential that fewer
        // things can reach is better.
        let mut session_id: Option<String> = None;

        while let Some(message) = socket.next().await {
            let text = match message? {
                Message::Text(t) => t,
                Message::Close(_) => break,
                // Ping/Pong/Frame do not carry events; tungstenite answers
                // pings itself.
                _ => continue,
            };

            let notification: Notification = match serde_json::from_str(&text) {
                Ok(n) => n,
                // Twitch sends session keepalives on the same socket. Ignoring
                // unparseable frames is correct, not lenient.
                Err(_) => continue,
            };

            // The first frame is always a session welcome carrying the session
            // id. Subscriptions are keyed by it, so it has to be captured before
            // any subscribe request can be sent.
            if let Some(id) = notification.metadata.session_id.as_deref()
                && session_id.is_none()
            {
                session_id = Some(id.to_string());
                log::debug!("twitch eventsub session {id}");

                // Subscriptions are keyed by that session id, so this is the
                // first point at which a subscribe request can be built. Doing
                // it here rather than before the welcome is why it has to wait.
                let request = self.subscribe_request(id, &broadcaster_id);
                let Ok(payload) = serde_json::to_string(&request) else {
                    log::warn!("twitch eventsub: could not build the subscribe request");
                    return Ok(());
                };

                if let Err(error) = socket.send(Message::Text(payload)).await {
                    log::warn!("twitch eventsub: could not subscribe: {error}");
                    return Ok(());
                }
            }

            match notification_to_event(&notification, &self.pricing) {
                Ok(event) => {
                    if let Some(words) = event.words {
                        log::info!(
                            "twitch {} from {} granted {words} words",
                            event.kind,
                            event.user.as_deref().unwrap_or("someone")
                        );
                    }
                    sink.on_event(event);
                }
                Err(_) => continue,
            }
        }

        let _ = sink; // the sink is per-connection; reconnects rebuild nothing
        let _ = socket.close(None).await;
        Ok(())
    }
}

/// Send a chat command's effect as an event.
///
/// Separate from the socket: `!yap extend 500` needs the same pricing path as
/// a sub, and going through `price_event` means a chat command cannot drift from
/// what a real event would grant.
pub fn command_event(words: u32, user: Option<String>) -> Event {
    let mut event = price_event("chat", "command", user, None, Some(words));
    event.note = Some("chat command".to_string());
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(message_type: &str, kind: &str) -> Notification {
        notification_with_body(message_type, kind, None)
    }

    fn notification_with_body(
        message_type: &str,
        kind: &str,
        event: Option<EventBody>,
    ) -> Notification {
        Notification {
            metadata: Metadata {
                message_type: message_type.to_string(),
                message_id: Some("m1".to_string()),
                session_id: None,
            },
            payload: Payload {
                subscription: Subscription {
                    kind: kind.to_string(),
                    user_id: Some("u1".to_string()),
                    event,
                },
            },
        }
    }

    #[test]
    fn a_hundred_gifted_subs_are_worth_a_hundred_times_one() {
        // The bug: the event body was never read, so the amount was always
        // missing, so a gift bomb and a single gift priced identically. A
        // streamer watching a hundred subs land on one line and the goal move
        // by fifty words has no way to tell that from a miscount.
        let mut pricing = TwitchPricing::default();
        pricing.words.insert("gift_sub".to_string(), 50);

        let one = notification_with_body(
            "notification",
            "channel.subscription.gift",
            Some(EventBody {
                cumulative_total: Some(1),
                ..EventBody::empty()
            }),
        );
        let hundred = notification_with_body(
            "notification",
            "channel.subscription.gift",
            Some(EventBody {
                cumulative_total: Some(100),
                ..EventBody::empty()
            }),
        );

        let one = notification_to_event(&one, &pricing).expect("priced");
        let hundred = notification_to_event(&hundred, &pricing).expect("priced");

        assert_eq!(one.words, Some(50));
        assert_eq!(
            hundred.words,
            Some(5_000),
            "a hundred gifted subs must be worth a hundred times one",
        );
    }

    #[test]
    fn a_gift_with_no_reported_size_worth_nothing_rather_than_a_full_one() {
        // Granting the flat rate to an event of unknown size is how a
        // notification we failed to read turns into free words.
        let pricing = TwitchPricing::default();
        let event = notification_to_event(
            &notification("notification", "channel.subscription.gift"),
            &pricing,
        )
        .expect("priced");

        assert_eq!(event.words, Some(0));
    }

    #[test]
    fn a_cheer_is_worth_its_bits() {
        let mut pricing = TwitchPricing::default();
        pricing.words.insert("cheer".to_string(), 1);

        let event = notification_to_event(
            &notification_with_body(
                "notification",
                "channel.cheer",
                Some(EventBody {
                    bits: Some(2_500),
                    ..EventBody::empty()
                }),
            ),
            &pricing,
        )
        .expect("priced");

        assert_eq!(event.amount, Some(2_500.0));
        assert_eq!(event.words, Some(2_500));
    }

    #[test]
    fn a_plain_subscribe_is_worth_one_subscribe_however_many_fields_it_carries() {
        // Not per-unit: a resubscribe is one event, and reading it as a
        // multiplier would let a months-long resub become a huge award.
        let event = notification_to_event(
            &notification_with_body(
                "notification",
                "channel.subscribe",
                Some(EventBody::empty()),
            ),
            &TwitchPricing::default(),
        )
        .expect("priced");

        assert_eq!(event.words, Some(100));
    }

    #[test]
    fn namespaced_types_become_short_kinds() {
        // Settings keys read `twitch:sub`, not
        // `twitch:channel.subscribe`.
        assert_eq!(short_kind("channel.subscribe"), "sub");
        assert_eq!(short_kind("channel.subscription.message"), "resub");
        assert_eq!(short_kind("channel.subscription.gift"), "gift_sub");
        assert_eq!(short_kind("channel.cheer"), "cheer");
        assert_eq!(short_kind("channel.raid"), "raid");
        assert_eq!(
            short_kind("channel.channel_points_custom_reward_redemption.add"),
            "redemption"
        );
    }

    #[test]
    fn an_unknown_type_keeps_its_full_name() {
        // Inventing a category for an event we do not know about would be worse
        // than admitting it.
        assert_eq!(short_kind("channel.poll.begin"), "channel.poll.begin");
    }

    #[test]
    fn a_notification_is_priced_from_its_kind() {
        let event = notification_to_event(
            &notification("notification", "channel.subscribe"),
            &TwitchPricing::default(),
        )
        .expect("priced");

        assert_eq!(event.kind, "sub");
        assert_eq!(event.platform, "twitch");
        assert_eq!(event.words, Some(100));
    }

    #[test]
    fn a_cheer_is_priced_per_bit() {
        let event = notification_to_event(
            &notification("notification", "channel.cheer"),
            &TwitchPricing::default(),
        )
        .expect("priced");

        // One word per bit, with no amount on the notification: zero until the
        // event body is joined in.
        assert_eq!(event.kind, "cheer");
        assert_eq!(event.words, Some(0));
    }

    #[test]
    fn non_notifications_are_rejected() {
        // Keepalives and subscription confirmations share this shape and must
        // not reach the goal.
        for kind in [
            "session_keepalive",
            "subscription_confirmation",
            "revocation",
        ] {
            let err = notification_to_event(
                &notification(kind, "channel.subscribe"),
                &TwitchPricing::default(),
            )
            .expect_err("rejected");
            assert_eq!(err, "not an event notification");
        }
    }

    #[test]
    fn an_unpriced_kind_grants_nothing() {
        let mut pricing = TwitchPricing::default();
        pricing.words.remove("sub");

        let event =
            notification_to_event(&notification("notification", "channel.subscribe"), &pricing)
                .expect("still an event");

        assert_eq!(event.words, None);
        assert_eq!(event.granted_words(), 0);
    }

    #[test]
    fn every_subscribed_type_has_a_default_price() {
        // A subscription type with no configured price is a socket message per
        // occurrence doing nothing. Worth catching in a test rather than in chat.
        let pricing = TwitchPricing::default();
        for event_type in SUBSCRIPTION_TYPES {
            let kind = short_kind(event_type);
            assert!(
                pricing.get(kind).is_some(),
                "no default price for {event_type} ({kind})"
            );
        }
    }

    #[test]
    fn a_cheer_of_1000_bits_adds_1000_words() {
        // The scenario the per-unit rate exists for.
        let event = price_event(
            "twitch",
            "cheer",
            Some("cheerer".into()),
            Some(1000.0),
            Some(1),
        );
        assert_eq!(event.words, Some(1000));
        assert_eq!(event.user.as_deref(), Some("cheerer"));
    }

    #[test]
    fn a_chat_command_uses_the_same_pricing_path() {
        let event = command_event(500, Some("mod".into()));
        assert_eq!(event.words, Some(500));
        assert_eq!(event.platform, "chat");
        assert_eq!(event.kind, "command");
    }

    #[tokio::test]
    async fn stopping_does_not_wait_out_the_backoff() {
        // Quitting should be immediate even when reconnecting.
        let platform = TwitchPlatform::new(
            "someone".into(),
            "token".into(),
            "client".into(),
            Default::default(),
        );

        assert!(!platform.stopping());
        platform.stop().await;
        assert!(platform.stopping(), "stop is visible to the reconnect loop");

        // The wake handle carries a stored permit, so a waiter arriving after
        // the stop still wakes rather than sleeping out the backoff.
        let wake = platform.wake.clone();
        tokio::time::timeout(Duration::from_secs(1), wake.notified())
            .await
            .expect("stop should wake the loop immediately");
    }

    #[test]
    fn credential_names_are_stable() {
        // Settings record the service name; the user half is fixed here.
        assert_eq!(KEYRING_USER, "user-token");
    }

    #[test]
    fn the_channel_and_token_are_carried() {
        let platform = TwitchPlatform::new(
            "someone".into(),
            "tok-123".into(),
            "client".into(),
            Default::default(),
        );

        assert_eq!(platform.channel(), "someone");
        assert!(platform.has_token());
        // An empty token is detectable before the socket is opened, which is
        // better than a 401 from Twitch mid-stream.
        let empty = TwitchPlatform::new(
            "someone".into(),
            String::new(),
            "client".into(),
            Default::default(),
        );
        assert!(!empty.has_token());
    }
}
