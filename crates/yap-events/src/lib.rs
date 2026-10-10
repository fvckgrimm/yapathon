//! Chat platforms, behind one interface.
//!
//! A yapathon's goal moves because something happened on a stream: someone
//! subscribed, cheered, raided. Those arrive as wildly different shapes per
//! platform, and this module is the seam that stops that difference reaching the
//! goal math.
//!
//! Every event becomes a [`Event`] with a platform, a kind, and a word value
//! looked up from settings. A platform that sends an event kind the streamer has
//! not priced produces `words: None`, not a default — silently paying out would
//! move the goal without anyone agreeing to it.
//!
//! Twitch is the only implementation in v1. It is also the clean one: EventSub
//! over WebSocket delivers subs, resubs, gift subs, bits, raids and redemptions
//! on a single socket, with a user token and no inbound endpoint. YouTube needs
//! a quota-limited poll and Kick needs app credentials, so both are work that
//! belongs behind this trait rather than in the core.

use std::sync::Arc;

use async_trait::async_trait;
use yap_core::goal::Event;

pub mod twitch;

pub use twitch::{TwitchPlatform, TwitchPricing};

/// Where normalised events are delivered.
pub trait EventSink: Send + Sync + 'static {
    /// One platform or chat event, already priced in words.
    fn on_event(&self, event: Event);
}

/// A chat platform that can extend a yapathon.
#[async_trait]
pub trait ChatPlatform: Send + Sync {
    /// Short name, matching the `<platform>:<kind>` prefix in settings.
    fn name(&self) -> &'static str;

    /// Begin listening. Runs until `stop` resolves.
    ///
    /// Reconnection is this method's business: a socket drops mid-stream, and a
    /// yapathon that quietly stops counting subs is a yapathon whose goal stops
    /// meaning anything.
    async fn run(&self, sink: Arc<dyn EventSink>) -> anyhow::Result<()>;

    /// Ask the platform to stop. Safe to call more than once.
    async fn stop(&self);
}

/// How many words one platform-native amount is worth.
///
/// The conversion is per-event-kind because a bit and a sub are not worth the
/// same. Bits arrive in thousands, gifts in tens, so a flat rate would make one
/// of them meaningless.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WordRate {
    /// Words per whole unit of the platform's amount.
    pub per_unit: u32,
}

impl WordRate {
    pub const fn per(words: u32) -> Self {
        Self { per_unit: words }
    }

    /// Words granted for `amount`, rounded down.
    ///
    /// Rounding down rather than to nearest: a fractional word is not a word,
    /// and rounding down means chat never gains from a fractional cheer.
    pub fn words_for(&self, amount: f64) -> u32 {
        if amount <= 0.0 {
            return 0;
        }
        let exact = amount * f64::from(self.per_unit);
        // `as u32` saturates in Rust, so a huge amount cannot wrap.
        exact as u32
    }
}

/// Look up the word value for an event, honouring the rate for its kind.
///
/// `flat` is the configured value for `<platform>:<kind>`. When a kind pays per
/// unit of platform amount — bits, mostly — the amount scales it.
pub fn price_event(
    platform: &str,
    kind: &str,
    user: Option<String>,
    amount: Option<f64>,
    configured: Option<u32>,
) -> Event {
    // A per-unit rate is expressed in settings as the value for one unit, and a
    // separate table scales by amount. Keeping one flat value per kind means a
    // streamer prices a cheer at "words per 100 bits" without the core needing
    // to know what a bit is.
    let words: Option<u32> = match (configured, amount) {
        // A per-unit kind with no amount is an event whose size we failed to
        // read. Granting the rate as though the amount were 1 would hand out a
        // single word for an unknown-size cheer, which is worse than nothing.
        (Some(per_unit), Some(amount)) if is_per_unit(kind) => {
            Some(WordRate::per(per_unit).words_for(amount))
        }
        (Some(_), None) if is_per_unit(kind) => Some(0),
        (Some(words), _) => Some(words),
        (None, _) => None,
    };

    Event {
        platform: platform.to_string(),
        kind: kind.to_string(),
        user,
        amount,
        words,
        note: None,
    }
}

/// Kinds whose configured value is a rate per unit rather than a flat grant.
///
/// Cheered bits is the only one: 100 bits and 1000 bits are both just "bits",
/// and pricing them identically would make big cheers pointless.
/// Whether a kind's configured value is *per unit* rather than a flat total.
///
/// A cheer is priced per bit and a gift per sub, because "100 gifted subs"
/// arrives as one notification carrying a quantity and has to be worth a
/// hundred times what one sub is worth. Pricing those flat makes a gift bomb
/// and a big cheer worth the same as the smallest of either, which reads as the
/// app being broken rather than as a pricing choice.
fn is_per_unit(kind: &str) -> bool {
    matches!(kind, "cheer" | "gift_sub")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn an_unpriced_event_grants_nothing() {
        // Not defaulting is the point: chat should not be able to move a goal
        // through an event kind the streamer never agreed to pay for.
        let e = price_event("twitch", "follow", Some("someone".into()), None, None);
        assert_eq!(e.words, None);
        assert_eq!(e.granted_words(), 0);
        assert_eq!(e.kind, "follow");
    }

    #[test]
    fn a_flat_value_applies_regardless_of_amount() {
        let e = price_event(
            "twitch",
            "resub",
            Some("someone".into()),
            Some(12.0),
            Some(100),
        );
        assert_eq!(e.words, Some(100));
    }

    #[test]
    fn cheer_scales_with_the_amount() {
        // 100 bits at one word per bit, then 1000.
        let small = price_event("twitch", "cheer", None, Some(100.0), Some(1));
        assert_eq!(small.words, Some(100));

        let large = price_event("twitch", "cheer", None, Some(1000.0), Some(1));
        assert_eq!(large.words, Some(1000));
    }

    #[test]
    fn fractional_amounts_round_down() {
        let e = price_event("twitch", "cheer", None, Some(150.5), Some(1));
        // 150 words, not 151: a fraction of a word is not a word.
        assert_eq!(e.words, Some(150));
    }

    #[test]
    fn a_zero_or_negative_amount_grants_nothing() {
        let e = price_event("twitch", "cheer", None, Some(0.0), Some(1));
        assert_eq!(e.words, Some(0));

        let e = price_event("twitch", "cheer", None, Some(-50.0), Some(1));
        assert_eq!(e.words, Some(0));
    }

    #[test]
    fn an_enormous_amount_saturates_rather_than_wrapping() {
        // `as u32` saturates in Rust, so a bad payload cannot wrap a count
        // negative and make the goal smaller.
        let e = price_event("twitch", "cheer", None, Some(1e30), Some(1));
        assert_eq!(e.words, Some(u32::MAX));
    }

    #[test]
    fn per_unit_rates_arithmetic() {
        assert_eq!(WordRate::per(10).words_for(1.0), 10);
        assert_eq!(WordRate::per(10).words_for(0.5), 5);
        assert_eq!(WordRate::per(10).words_for(9.99), 99);
        assert_eq!(WordRate::per(10).words_for(0.0), 0);
    }

    #[derive(Default)]
    struct CollectSink {
        events: parking_lot::Mutex<Vec<Event>>,
    }

    impl EventSink for CollectSink {
        fn on_event(&self, event: Event) {
            self.events.lock().push(event);
        }
    }

    #[tokio::test]
    async fn a_sink_receives_events_from_any_platform() {
        let sink = Arc::new(CollectSink::default());
        let dyn_sink: Arc<dyn EventSink> = sink.clone();

        dyn_sink.on_event(price_event("twitch", "sub", None, None, Some(100)));
        dyn_sink.on_event(price_event("youtube", "superchat", None, None, Some(250)));

        let events = sink.events.lock();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].platform, "twitch");
        assert_eq!(events[1].platform, "youtube");
    }
}
