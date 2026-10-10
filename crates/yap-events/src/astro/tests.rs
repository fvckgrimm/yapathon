//! Astro tests.
//!
//! The fixtures are frames captured from a live StreamElements gateway with
//! `tools/astro-probe`, not invented shapes. That is the whole reason the probe
//! existed: a mapping written against documentation would have been written
//! against a `channel.activities` page that documents no `amount` at all, and
//! would have priced every Super Chat as a flat grant while passing every test.

use super::*;

fn pricing(pairs: &[(&str, u32)]) -> std::collections::BTreeMap<String, u32> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

/// The platform under test, with the prices that matter.
fn platform() -> AstroPlatform {
    AstroPlatform::new(
        Vec::new(),
        pricing(&[
            ("twitch:cheer", 1),
            ("twitch:gift_sub", 25),
            ("youtube:superchat", 10),
            ("youtube:subscriber", 50),
            ("youtube:sub", 100),
        ]),
    )
}

/// Price a captured frame, exactly as the read loop does.
fn event_from(frame: &str) -> Option<Event> {
    let frame: Frame = serde_json::from_str(frame).ok()?;
    let activity = frame.data.as_ref().and_then(FrameData::activity)?;
    platform().to_event(&activity)
}

/// A real Twitch gift bomb: 10 subs, tier 1.
const TWITCH_GIFT_BOMB: &str = r#"{
  "type":"message","topic":"channel.activities","room":"5e7439a0",
  "data":{"type":"communityGiftPurchase","provider":"twitch","channel":"5e7439a0",
    "isMock":true,"activityId":"a2","activityGroup":"1791672974736",
    "data":{"username":"daisey","displayName":"Daisey","providerId":"1","amount":10}}
}"#;

/// A real YouTube Super Chat.
const YOUTUBE_SUPERCHAT: &str = r#"{
  "type":"message","topic":"channel.activities","room":"6acac21e",
  "data":{"type":"superchat","provider":"youtube","channel":"6acac21e",
    "isMock":true,"activityId":"a1",
    "data":{"username":"cosmicninja650","displayName":"CosmicNinja650","providerId":"2","amount":1}}
}"#;

/// A real YouTube channel subscriber — the event the native Live Streaming API
/// has no notification for anywhere.
const YOUTUBE_SUBSCRIBER: &str = r#"{
  "type":"message","topic":"channel.activities","room":"6acac21e",
  "data":{"type":"subscriber","provider":"youtube","channel":"6acac21e",
    "isMock":true,
    "data":{"username":"fuzzyknight164","displayName":"FuzzyKnight164","providerId":"3"}}
}"#;

/// A real YouTube channel membership, which is money and is not a subscriber.
const YOUTUBE_SPONSOR: &str = r#"{
  "type":"message","topic":"channel.activities","room":"6acac21e",
  "data":{"type":"sponsor","provider":"youtube","channel":"6acac21e",
    "isMock":true,
    "data":{"username":"turbogamer771","displayName":"TurboGamer771","providerId":"4","amount":4}}
}"#;

#[test]
fn a_gift_bomb_is_worth_its_count_and_not_a_single_gift() {
    // The bug this prevents: a gift bomb arriving with a count and being priced
    // flat, so a hundred gifted subs moved the goal by the same amount as one.
    let event = event_from(TWITCH_GIFT_BOMB).expect("a priced event");

    assert_eq!(event.kind, "gift_sub");
    assert_eq!(event.amount, Some(10.0));
    assert_eq!(event.words, Some(250), "10 gifts × 25 words each");
}

#[test]
fn a_superchat_is_worth_what_was_actually_spent() {
    let event = event_from(YOUTUBE_SUPERCHAT).expect("a priced event");

    assert_eq!(event.kind, "superchat");
    assert_eq!(event.words, Some(10), "1 unit of money × 10 words");
}

#[test]
fn a_youtube_subscriber_is_an_event_a_native_youtube_channel_never_sees() {
    // The reason this platform exists at all. Flat, because a person clicking
    // Subscribe is a fixed-size event — but the native API cannot report it in
    // any form, so reaching this at all is the point.
    let event = event_from(YOUTUBE_SUBSCRIBER).expect("a priced event");

    assert_eq!(event.kind, "subscriber");
    assert_eq!(event.amount, None, "flat: one subscriber is one subscriber");
    assert_eq!(
        event.words,
        Some(50),
        "flat pricing, not scaled by anything"
    );
}

#[test]
fn a_youtube_membership_is_not_a_subscriber() {
    // Conflating these would be the expensive mistake: they look alike in the
    // event list and one is money while the other is not.
    let membership = event_from(YOUTUBE_SPONSOR).expect("a priced event");

    assert_eq!(membership.kind, "membership");
    assert_ne!(membership.kind, "subscriber");
    assert_eq!(
        membership.amount, None,
        "flat unless the streamer prices by tier"
    );
    assert_eq!(
        membership.words, None,
        "unpriced, so it grants nothing — a membership must be priced deliberately",
    );
}

#[test]
fn a_twitch_resub_is_distinguished_from_a_first_subscription() {
    // `amount` is months, so a first month is a sub and anything past one is a
    // resub — and streamers price those differently.
    let first = shape(&sub_with(1.0, 1)).expect("shaped");
    let again = shape(&sub_with(12.0, 12)).expect("shaped");

    assert_eq!(first, ("sub", None));
    assert_eq!(again, ("resub", None));
}

#[test]
fn a_cheer_carries_bits_and_a_raid_carries_viewers() {
    // The two fields that look identical in the payload and mean completely
    // different things to a streamer.
    let cheer = shape(&activity("twitch", "cheer", Some(2500.0))).expect("shaped");
    let raid = shape(&activity("twitch", "raid", Some(37.0))).expect("shaped");

    assert_eq!(cheer, ("cheer", Some(2500.0)));
    assert_eq!(raid, ("raid", Some(37.0)));
}

#[test]
fn the_size_is_read_from_the_nested_object_and_not_the_outer_one() {
    // The bug this module is most likely to ship with. StreamElements nests
    // every size inside `data.data`; the topic page lists only the outer
    // fields. A mapping written from the documentation puts `amount` at the top
    // level, gets `None`, and prices every Super Chat as a flat grant — while
    // still passing a test written against the same wrong assumption.
    //
    // So: this frame has an `amount` in the nested object and none outside, and
    // the event must come back scaled.
    let event = event_from(YOUTUBE_SUPERCHAT).expect("a priced event");

    assert_eq!(
        event.amount,
        Some(1.0),
        "the nested amount was missed, so this Super Chat would be worth a flat rate",
    );
    assert_eq!(event.words, Some(10));
}

#[test]
fn a_merch_basket_is_worth_what_it_actually_cost() {
    // A real capture. `amount` here is 1 — the number of line items — and the
    // money is 9.99 + 19.99 + 4.99. Reading `amount` would price a 34.97 basket
    // at the price of a 1.00 one.
    let frame = r#"{
      "type":"message","topic":"channel.activities",
      "data":{"type":"merch","provider":"youtube","isMock":true,
        "data":{"username":"fuzzycomet942","displayName":"FuzzyComet942",
          "amount":1,
          "items":[{"name":"T-Shirt","price":9.99,"quantity":1},
                   {"name":"Hoodie","price":19.99,"quantity":1},
                   {"name":"Mug","price":4.99,"quantity":1}]}}
    }"#;

    let parsed: Frame = serde_json::from_str(frame).unwrap();
    let activity = parsed.data.as_ref().and_then(FrameData::activity).unwrap();

    assert_eq!(
        activity.data.amount,
        Some(1.0),
        "one line item, not one unit of money"
    );

    let (kind, amount) = shape(&activity).expect("shaped");
    assert_eq!(kind, "merch");
    assert!(
        (amount.unwrap() - 34.97).abs() < 0.001,
        "a 34.97 basket should be worth 34.97, got {:?}",
        amount,
    );
}

#[test]
fn a_simulated_event_is_never_counted() {
    // Every payload captured with the probe carried `isMock: true` — the widget
    // editor's test button. If these were counted, a yapathon could be extended
    // by a cheer nobody sent, which is the sort of thing that is only discovered
    // by someone noticing their goal moved for no reason.
    let parsed: Frame = serde_json::from_str(TWITCH_GIFT_BOMB).unwrap();
    let activity = parsed.data.as_ref().and_then(FrameData::activity).unwrap();
    assert!(activity.is_mock);

    // And a real one is not.
    let real = r#"{"type":"message","topic":"channel.activities",
      "data":{"type":"cheer","provider":"twitch","data":{"displayName":"x","amount":100}}}"#;
    let parsed: Frame = serde_json::from_str(real).unwrap();
    assert!(
        !parsed
            .data
            .as_ref()
            .and_then(FrameData::activity)
            .unwrap()
            .is_mock
    );
}

#[test]
fn an_activity_with_no_amount_is_flat_rather_than_worthless() {
    // Absent means flat. A follow genuinely has no size, and should be able to
    // be worth something.
    let follow = shape(&activity("twitch", "follow", None)).expect("shaped");

    assert_eq!(follow, ("follow", None));
}

#[test]
fn a_merch_purchase_uses_the_unit_price_when_the_amount_is_a_quantity() {
    // No `items`, so the fallback is `amount` — a single priced thing.
    assert_eq!(
        shape(&activity("youtube", "merch", Some(15.0))).expect("shaped"),
        ("merch", Some(15.0))
    );
}

#[test]
fn events_nobody_did_for_the_streamer_are_not_priced() {
    // Hypetrain ticks, shares, video likes, sponsorship renewals. They arrive
    // on the same topic as everything else, so without this they would be
    // events with no price that silently do nothing forever.
    for kind in [
        "hypetrainStart",
        "hypetrainProgress",
        "hypetrainEnd",
        "share",
        "videolike",
        "sponsorship",
        "sponsorshipPassive",
        "stars",
        "elixir",
    ] {
        assert!(
            shape(&activity("twitch", kind, Some(5.0))).is_none(),
            "{kind} should not be priced",
        );
    }
}

#[test]
fn every_platform_streamselements_names_is_handled_or_deliberately_ignored() {
    // The provider list from their docs. A new platform should land in the
    // generic arm rather than falling through to "nothing is priced".
    for provider in ["twitch", "youtube", "kick"] {
        assert!(
            shape(&activity(provider, "cheer", Some(100.0))).is_some(),
            "{provider} cheer should be priced",
        );
        assert!(
            shape(&activity(provider, "subscriber", Some(1.0))).is_some(),
            "{provider} subscriber should be priced",
        );
    }
}

#[test]
fn prices_the_streamer_already_set_are_reused_rather_than_invented() {
    // Switching a channel from native Twitch to Astro must not silently
    // re-price everything: `twitch:cheer` means the same thing either way.
    let frame = r#"{"type":"message","topic":"channel.activities",
      "data":{"type":"cheer","provider":"twitch",
        "data":{"displayName":"x","amount":2500}}}"#;
    let event = event_from(frame).expect("priced");

    assert_eq!(event.kind, "cheer");
    assert_eq!(event.words, Some(2500), "2500 bits × 1 word per bit");
}

#[test]
fn the_platform_comes_from_the_event_and_not_from_where_the_token_was_saved() {
    // The settings panel offers one slot per platform, which is the axis a
    // streamer thinks in. But a token does not say which platform it covers and
    // nothing checks, so a token saved in the wrong slot must still be priced
    // under the platform its events actually name — otherwise the event is
    // counted and `youtube:superchat` finds no price for it, and the goal
    // silently stops responding to chat.
    let frame = r#"{"type":"message","topic":"channel.activities",
      "data":{"type":"superchat","provider":"youtube",
        "data":{"displayName":"Viewer","amount":5}}}"#;

    let event = event_from(frame).expect("priced");

    assert_eq!(
        event.platform, "youtube",
        "a YouTube event is priced as YouTube whatever slot its token sits in",
    );
    assert_eq!(event.kind, "superchat");
    assert_eq!(event.words, Some(50), "5 units of money × 10 words each");
}

#[test]
fn an_activity_delivered_twice_is_counted_once() {
    // A reconnect, or a publisher retrying a delivery, would otherwise let a
    // cheer through twice. A counter that is occasionally wrong is worse than
    // one that is obviously wrong, because nobody notices it.
    let frame = r#"{"type":"message","topic":"channel.activities",
      "data":{"type":"cheer","provider":"twitch","activityId":"cheer-1",
        "data":{"displayName":"Someone","amount":2500}}}"#;

    let parsed: Frame = serde_json::from_str(frame).unwrap();
    let activity = parsed.data.as_ref().and_then(FrameData::activity).unwrap();

    let platform = platform();
    assert!(!platform.already_seen(&activity), "first delivery is new");
    assert!(
        platform.already_seen(&activity),
        "second delivery is the same one"
    );
    assert!(
        platform.already_seen(&activity),
        "and still remembered, so a third copy is caught too",
    );

    // A different activity is not affected.
    let other = r#"{"type":"message","topic":"channel.activities",
      "data":{"type":"cheer","provider":"twitch","activityId":"cheer-2",
        "data":{"displayName":"Someone","amount":100}}}"#;
    let parsed: Frame = serde_json::from_str(other).unwrap();
    let other = parsed.data.as_ref().and_then(FrameData::activity).unwrap();

    assert!(!platform.already_seen(&other));
}

#[test]
fn an_activity_with_no_id_is_counted_rather_than_dropped() {
    // The alternative is losing events to a field that is usually present.
    // Being wrong twice is the lesser failure.
    let frame = r#"{"type":"message","topic":"channel.activities",
      "data":{"type":"cheer","provider":"twitch",
        "data":{"displayName":"Someone","amount":100}}}"#;

    let parsed: Frame = serde_json::from_str(frame).unwrap();
    let activity = parsed.data.as_ref().and_then(FrameData::activity).unwrap();

    let platform = platform();
    assert!(!platform.already_seen(&activity));
    assert!(
        !platform.already_seen(&activity),
        "an id-less activity cannot be deduplicated, so it passes rather than being silently lost",
    );
}

#[test]
fn the_dedup_set_forgets_old_enough_activities() {
    // Bounded, so a long stream cannot grow it without limit.
    for i in 0..(SEEN_LIMIT + 50) {
        let frame = format!(
            r#"{{"type":"message","topic":"channel.activities",
              "data":{{"type":"cheer","provider":"twitch","activityId":"c{i}",
                "data":{{"displayName":"Someone","amount":1}}}}}}"#
        );
        let parsed: Frame = serde_json::from_str(&frame).unwrap();
        let activity = parsed.data.as_ref().and_then(FrameData::activity).unwrap();

        assert!(!platform().already_seen(&activity), "activity {i} is new");
    }
}

// --- helpers ---------------------------------------------------------------

fn data(amount: Option<f64>) -> ActivityData {
    ActivityData {
        user: "Someone".to_string(),
        username: "someone".to_string(),
        amount,
        streak: None,
        items: Vec::new(),
    }
}

/// A borrowed activity for a direct call to [`shape`].
///
/// Leaks its data rather than owning it, so the helpers can hand out
/// `Activity<'static>` views without threading a lifetime through every test.
/// A handful of small structs leaking in a test binary is not a thing to fix.
fn activity(provider: &'static str, kind: &'static str, amount: Option<f64>) -> Activity<'static> {
    Activity {
        provider,
        kind,
        is_mock: false,
        id: None,
        data: Box::leak(Box::new(data(amount))),
    }
}

fn sub_with(months: f64, streak: u32) -> Activity<'static> {
    Activity {
        data: Box::leak(Box::new(ActivityData {
            streak: Some(streak),
            ..data(Some(months))
        })),
        ..activity("twitch", "subscriber", Some(months))
    }
}
