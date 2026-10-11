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
///
/// Every captured frame here is `isMock: true`, because that is all the widget
/// simulator produces — so events come back labelled `(mock)`. The words are
/// identical either way; only the label differs, and the tests that care about
/// labelling say so.
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

    assert_eq!(
        event.kind, "gift_sub (mock)",
        "captured frames are simulated"
    );
    assert_eq!(event.amount, Some(10.0));
    assert_eq!(event.words, Some(250), "10 gifts × 25 words each");
}

#[test]
fn a_superchat_is_worth_what_was_actually_spent() {
    let event = event_from(YOUTUBE_SUPERCHAT).expect("a priced event");

    assert_eq!(
        event.kind, "superchat (mock)",
        "captured frames are simulated"
    );
    assert_eq!(event.words, Some(10), "1 unit of money × 10 words");
}

#[test]
fn a_youtube_subscriber_is_an_event_a_native_youtube_channel_never_sees() {
    // The reason this platform exists at all. Flat, because a person clicking
    // Subscribe is a fixed-size event — but the native API cannot report it in
    // any form, so reaching this at all is the point.
    let event = event_from(YOUTUBE_SUBSCRIBER).expect("a priced event");

    assert_eq!(
        event.kind, "subscriber (mock)",
        "captured frames are simulated"
    );
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

    assert_eq!(
        membership.kind, "membership (mock)",
        "captured frames are simulated"
    );
    assert_ne!(
        membership.kind, "subscriber (mock)",
        "a membership and a subscriber look alike in the event list, and one is money",
    );
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
fn an_accepted_mock_is_priced_correctly_and_labelled_as_fake() {
    // The local testing mode. Two things have to be true at once, and they pull
    // against each other: the words must be right, because the point is testing
    // the chain, and the label must say the event is fake, because a test
    // session indistinguishable from a real one is worse than no test mode.
    //
    // So the relabelling happens *after* pricing — `kind` is display only, and
    // `apply_event` reads `words` and nothing else.
    let frame = r#"{"type":"message","topic":"channel.activities",
      "data":{"type":"cheer","provider":"twitch","isMock":true,
        "data":{"displayName":"Someone","amount":2500}}}"#;

    let parsed: Frame = serde_json::from_str(frame).unwrap();
    let activity = parsed.data.as_ref().and_then(FrameData::activity).unwrap();

    let event = platform().to_event(&activity).expect("priced");

    assert_eq!(event.words, Some(2500), "2500 bits × 1 word per bit");
    assert_eq!(event.platform, "twitch", "still priced as Twitch");
    assert_eq!(
        event.kind, "cheer (mock)",
        "and visibly not something a viewer sent",
    );
}

#[test]
fn a_real_event_is_not_labelled_as_a_mock() {
    let frame = r#"{"type":"message","topic":"channel.activities",
      "data":{"type":"cheer","provider":"twitch",
        "data":{"displayName":"Someone","amount":2500}}}"#;

    let parsed: Frame = serde_json::from_str(frame).unwrap();
    let activity = parsed.data.as_ref().and_then(FrameData::activity).unwrap();

    let event = platform().to_event(&activity).expect("priced");

    assert_eq!(
        event.kind, "cheer",
        "a real event must never be dressed up as a test one",
    );
}

#[test]
fn a_captured_payload_reaches_a_running_session_as_words() {
    // The whole chain, which no test above covers: a real captured frame, shaped,
    // priced, handed to a session, and landing as words on the goal.
    //
    // The gap this closes is real. Every other test here stops at the `Event`,
    // and the session half was only ever exercised by chat commands — so a
    // payload arriving, being shaped and priced correctly, and then going
    // nowhere would look identical to a working integration from every angle
    // except "does the number move".
    let frame = r#"{"type":"message","topic":"channel.activities",
      "data":{"type":"cheer","provider":"twitch","isMock":true,
        "data":{"displayName":"Someone","amount":2500}}}"#;

    let parsed: Frame = serde_json::from_str(frame).unwrap();
    let activity = parsed.data.as_ref().and_then(FrameData::activity).unwrap();

    let event = platform().to_event(&activity).expect("priced");
    assert_eq!(event.words, Some(2500));

    // Extend, not Fixed. In Fixed an event adds score and the goal stays put,
    // which is that mode's whole meaning — pinned by the test below.
    let mut session = yap_core::session::Session::new(yap_core::session::SessionConfig::new(
        yap_core::goal::GoalMode::Extend { base_words: 10_000 },
        None,
    ));
    session.start();

    let granted = session.apply_event(&event);
    assert_eq!(granted, 2500, "the cheer moved the goal");

    let snapshot = session.snapshot();
    assert_eq!(snapshot.target, 12_500, "and extended the finish line");
    assert_eq!(snapshot.bonus_total, 2500, "counted as coming from chat");
    assert_eq!(
        snapshot.words, 0,
        "and kept apart from what was spoken, which is why the two exist separately",
    );
}

#[test]
fn a_fixed_yapathon_leaves_the_finish_line_alone_and_scores_instead() {
    // The second way "nothing happened" turns out to be reachable, and from the
    // overlay it looks identical to a broken integration: a number that does not
    // move. In fixed mode events add score and never goal — that is the mode's
    // meaning, not a fault — but it is worth pinning so it is rediscovered as
    // design rather than as a bug.
    let frame = r#"{"type":"message","topic":"channel.activities",
      "data":{"type":"cheer","provider":"twitch","isMock":true,
        "data":{"displayName":"Someone","amount":2500}}}"#;

    let parsed: Frame = serde_json::from_str(frame).unwrap();
    let activity = parsed.data.as_ref().and_then(FrameData::activity).unwrap();
    let event = platform().to_event(&activity).expect("priced");

    let mut session = yap_core::session::Session::new(yap_core::session::SessionConfig::new(
        yap_core::goal::GoalMode::Fixed { words: 10_000 },
        None,
    ));
    session.start();
    session.apply_event(&event);

    let snapshot = session.snapshot();
    assert_eq!(snapshot.target, 10_000, "the finish line does not move");
    assert_eq!(snapshot.score, 2500, "the event is worth score instead");
}

#[test]
fn a_merch_basket_whose_amount_is_zero_is_still_worth_something() {
    // A second captured basket, and the sharper version of the bug: here
    // `amount` is **0**, so reading it would value a 34.97 purchase at nothing
    // at all — not at a slightly wrong number, but at zero. The price is only
    // ever in `items`, and a streamer selling merch would watch it happen.
    let frame = r#"{
      "type":"message","topic":"channel.activities","conn":"twitch",
      "data":{"type":"merch","provider":"twitch","isMock":true,
        "data":{"username":"fuzzyraven128","displayName":"FuzzyRaven128",
          "amount":0,
          "items":[{"name":"T-Shirt","price":9.99,"quantity":1},
                   {"name":"Hoodie","price":19.99,"quantity":1},
                   {"name":"Mug","price":4.99,"quantity":1}]}}
    }"#;

    let parsed: Frame = serde_json::from_str(frame).unwrap();
    let activity = parsed.data.as_ref().and_then(FrameData::activity).unwrap();

    assert_eq!(
        activity.data.amount,
        Some(0.0),
        "the field that is not the price"
    );

    let (kind, amount) = shape(&activity).expect("shaped");
    assert_eq!(kind, "merch");
    assert_eq!(
        amount,
        Some(34.97),
        "a 34.97 basket is worth 34.97, and worth nothing if amount is trusted",
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
fn a_simulated_event_is_dropped_unless_test_mode_is_on() {
    // The rule the read loop enforces, checked without touching process-wide
    // state: fakes never reach pricing by default, and Test Mode lets exactly
    // the fakes through — real events are unaffected either way.
    assert!(mock_dropped(true, false));
    assert!(!mock_dropped(true, true));
    assert!(!mock_dropped(false, false));
    assert!(!mock_dropped(false, true));
}

#[test]
fn the_test_mode_switch_takes_effect_without_a_restart() {
    // The UI toggle flips this on a running connection, so setting it must be
    // observable immediately rather than only at the next launch. Restored at
    // the end: the flag is process-wide and a test that leaves it on would let
    // fake events through for every test after it.
    let was = mock_events_allowed();

    set_mock_events_allowed(true);
    assert!(
        mock_events_allowed(),
        "flipping Test Mode on must read back on"
    );

    set_mock_events_allowed(false);
    assert!(
        !mock_events_allowed(),
        "and flipping it off must read back off"
    );

    set_mock_events_allowed(was);
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
#[test]
fn debug_extend() {
    use yap_core::goal::{Event, GoalMode};
    use yap_core::session::{Session, SessionConfig};
    let ev = Event {
        platform: "twitch".into(),
        kind: "cheer".into(),
        user: None,
        amount: Some(2500.0),
        words: Some(2500),
        note: None,
    };
    let mut s = Session::new(SessionConfig::new(
        GoalMode::Extend { base_words: 10_000 },
        None,
    ));
    s.start();
    let snap = s.snapshot();
    eprintln!(
        "before: target={} words={} capped={}",
        snap.target, snap.words, snap.capped
    );
    let g = s.apply_event(&ev);
    let snap = s.snapshot();
    eprintln!(
        "granted={} after: target={} words={} bonus={}",
        g, snap.target, snap.words, snap.bonus_total
    );
}
