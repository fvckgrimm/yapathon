# YouTube

Planning notes, written against the actual API reference rather than from
memory. Quota numbers are the reason the shape of this is what it is, so they
come first.

## Quota, which decides everything

The YouTube Data API default allocation is **10,000 units/day**, resetting at
midnight Pacific. From the
[quota table](https://developers.google.com/youtube/v3/determine_quota_cost):

| Method | Cost |
| --- | --- |
| `liveChatMessages.list` | 5 |
| `liveChatMessages.streamList` | 5 |
| `liveChatMessages.insert` | 20 |
| `liveBroadcasts.list` | 1 |

That 5-unit cost is per *call*, and a polling client calls constantly. The
arithmetic is unforgiving:

| Poll interval | Calls/day | Hours of stream covered |
| --- | --- | --- |
| 5s (what YouTube's own `pollingIntervalMillis` usually says) | 2,000 | **2.8** |
| 10s | 2,000 | 5.6 |
| 15s | 2,000 | 8.3 |
| 20s | 2,000 | 11.1 |

Polling at the interval YouTube suggests exhausts the day's quota two and a
half hours into an evening stream. This is the single most important thing
about integrating YouTube, and it is invisible until it silently stops counting
at hour three.

**`streamList` is the answer.** It is a server-streaming endpoint — the
response body stays open and YouTube writes new messages into it as they
arrive, the way a server-sent-events feed works. The 5 units are charged when
the connection is **established**, not per message. A six-hour stream held open
on one connection costs 5 units. Even reconnecting every time the network blips
is noise against a 10,000-unit budget.

So: `streamList` as the primary transport, `list` only as the fallback for
resuming after a long disconnect or when the app starts mid-stream.

### Reading the stream

The endpoint returns a chunked response that is a sequence of concatenated
`liveChatMessageListResponse` JSON objects with no framing between them — not
newline-delimited, so a line reader will not work. `serde_json` has this
exactly:

```rust
let stream = serde_json::Deserializer::from_reader(response_body).into_iter::<LiveChatMessageListResponse>();
for page in stream { /* ... */ }
```

Each page carries its own `nextPageToken`. Reconnecting with the last token
received resumes from that point, which is how reconnection works without
gaps or double-counting.

## Events, and why this needs no pricing code

Every paid and membership event on YouTube arrives through the same
`liveChatMessages` feed, distinguished by `snippet.type`. Mapping them onto the
existing `amount`-scales rule:

| `snippet.type` | our kind | `amount` we send | price behaviour |
| --- | --- | --- | --- |
| `superChatEvent` | `superchat` | `superChatDetails.amountMicros` | scales with money spent |
| `superStickerEvent` | `supersticker` | `superStickerDetails.amountMicros` | scales |
| `newSponsorEvent` | `membership` | `None` | flat |
| `membershipGiftingEvent` | `gift_membership` | `membershipGiftingDetails.giftMembershipsCount` | scales by count |
| `giftMembershipReceivedEvent` | `gift_membership_received` | `None` | flat |
| `memberMilestoneChatEvent` | `member_milestone` | `None` | flat |
| `textMessageEvent` | `chat:command` | — | chat commands |

Ignored: `tombstone`, `chatEndedEvent`, `userBannedEvent`, `pollEvent`,
`giftEvent`, `sponsorOnlyModeStartedEvent`, `sponsorOnlyModeEndedEvent`.

**Nothing here needs a change to `price_event`.** A Super Chat sends its
`amountMicros`, so $5 is five times $1. Gifting fifty memberships sends
`giftMembershipsCount`, so a gift bomb is worth fifty memberships. A plain
membership sends nothing and is flat. This is the case the rule was written for
— the sender knows what its own amount means, so adding a platform is a matter
of reading a JSON field, not of teaching the shared code what a Super Chat is.

`amountMicros` is micros of the currency, so $1.00 is `1_000_000`. A sensible
default rate is on the order of "words per dollar", but that is a streamer
decision and belongs in `settings.event_words` like every other price.

### What YouTube genuinely does not have

- **Subscribers.** There is no follow notification anywhere in the Live
  Streaming API. YouTube is not telling anybody who pressed Subscribe during
  your stream. A YouTube-only yapathon cannot count new follows, full stop.
- **Raids and hosts.** No such concept on the platform.
- **Resubs**, as distinct from new memberships — the nearest thing is
  `memberMilestoneChatEvent`, which reports `memberMonth`.

Those are facts about YouTube rather than gaps in the work, so they belong in
the UI when a YouTube session is running: the run should say what it is not
counting, rather than silently under-reporting.

## OAuth

Read needs `https://www.googleapis.com/auth/youtube.readonly`. Posting — which
we would want for answering `!yap` in chat — needs
`https://www.googleapis.com/auth/youtube.force-ssl` and costs 20 units a
message, so about 500 replies a day. Cheap enough to be worth having, since it
removes the need for a separate bot bridge.

Setup, for whoever owns the channel:

1. A Google Cloud project, with the YouTube Data API v3 enabled.
2. OAuth consent screen. **If this is left in "Testing" mode, refresh tokens
   expire after seven days.** Publishing the app, or adding the channel owner as
   a test user, both avoid a token that quietly rots every week.
3. An OAuth client. Desktop type is fine — the loopback redirect works.
4. Store the client id, client secret and refresh token in the OS credential
   store alongside the Twitch token, never in `settings.toml`.

The app then exchanges the refresh token for an access token itself, on a timer,
with no browser involved after the first consent.

## `!yap` in chat, natively

Because we hold a refresh token with `youtube.force-ssl`, we can post to live
chat through `liveChatMessages.insert` at 20 units a message. No Streamer.bot,
no second program, no bot account to moderate. The same trick works on Twitch
through Helix, so chat commands come free on both platforms from an
integration we already need.

This is the cleanest answer to "make `!yap` work everywhere": it comes from the
same credential that makes the events work.

## StreamElements — measured, not assumed

Originally this section said StreamElements tips meant an undocumented REST poll,
which was wrong: StreamElements publishes an official WebSocket gateway
("Astro") at `wss://astro.streamelements.com/`, authenticated by a JWT copied
from their dashboard. No OAuth, no Cloud project, no quota, and no OBS source —
it is a socket from our process, not a hosted widget.

`tools/astro-probe` measured it. **The amounts are present**, on
`channel.activities`, in a nested `data.data` the topic page does not document:

| activity | `data.data` |
| --- | --- |
| `cheer` | `amount` = bits |
| `raid` / `host` | `amount` = viewers |
| `superchat` | `amount` = money (currency not on this topic) |
| `communityGiftPurchase` | `amount` = count of gifted subs |
| `subscriber` / `sponsor` | `amount` = months, plus `tier` |

So the gateway can price events proportionally, exactly as the native APIs can,
and it covers twitch, youtube and kick on one connection with no quota ceiling —
which the native YouTube path very much does have.

What remains unproven: every event in the test dump was `isMock: true`, so the
schema is confirmed but real traffic is not; and no YouTube event was captured,
so the one thing native YouTube *cannot* do — subscriber notifications — is
still unverified.

## StreamElements tips

The honest version: this is a smaller deal than it looks, and it is worth
doing late or not at all.

- The endpoint is `https://api.streamelements.com/kappa/v2/tips/{channel}`,
  authenticated with a JWT derived from the overlay API key. It is
  **undocumented** — the official developer portal is a mostly-empty Stoplight
  workspace, and the only real documentation is the community's
  [`c4ldas/streamelements-api`](https://github.com/c4ldas/streamelements-api).
- It is a **poll**, not a push. There is no subscription. Every tip costs a
  request.
- It requires the streamer to have a StreamElements account with tipping
  enabled.

None of those break the no-external-dependency rule — nothing has to be
installed — so it is allowed, unlike the Streamer.bot relay that was rejected.
But it is a second auth path, a second polling loop, and an unofficial API,
for an event type many streamers do not use. It should go through the same
`ChatPlatform` door as everything else, as `streamelements:tip` with the tip
amount as `amount`, so the pricing rule handles it unchanged.

**Recommendation: YouTube first, StreamElements tips after, and only if the
streamer in question actually takes SE tips.**

## About marathon-timer

Worth reading for event semantics; not worth copying as an architecture.

It runs as a **StreamElements custom widget**. It listens for the
`onEventReceived` browser event and uses `SE_API.store` for persistence — which
means StreamElements holds the Twitch and YouTube connections and relays the
events into the overlay page. The state lives on StreamElements' servers, not
on the streamer's machine.

That is precisely the relay shape we rejected for Streamer.bot, arrived at by a
different route: it works well, and it works *because* someone else's server is
in the middle. Its `youtube_subscriber-latest` events exist for the same reason
— YouTube does not expose follows, so StreamElements is supplying them from
somewhere else.

What is genuinely reusable is its pricing surface: one "worth" value per event
kind, with gifted variants branching off their non-gifted counterparts, and a
flat `timerChatTipWorth` applied to both cheers and Super Chats. Our version
generalises that into one rule, which is why the table above needs no new code.

## Built: Astro as a third platform

`crates/yap-events/src/astro.rs`, off by default, alongside rather than instead
of both native paths. Configured in Settings → Chat setup; one token per platform
account; prices shared with EventSub because both name events
`<platform>:<kind>`.

Two things the implementation learned from the captured frames rather than the
documentation:

- **Every size is nested.** `amount` lives in `frame.data.data`, not in
  `frame.data`. Their topic page lists only the outer fields, so a mapping
  written from the docs finds `None` and prices every Super Chat as a flat grant
  — while passing a test written against the same wrong assumption. Four tests
  fail if the nested read is removed.
- **`isMock` needs an explicit rename.** Serde maps `is_mock` to a field called
  `is_mock`; the payload spells it `isMock`; `#[serde(default)]` then supplies
  `false` silently, with no error, so every simulated cheer counts as though a
  viewer sent it.

Also: activities are deduplicated by `activityId`, and `amount` on a merch
activity is the number of line items, not money — the money is
`sum(price * quantity)` across `items`.

## Implementation

New files, all inside `yap-events`:

- `youtube/auth.rs` — refresh-token exchange and the expiry timer
- `youtube/model.rs` — the `liveChatMessage` subset we actually read
- `youtube/stream.rs` — `streamList` reader with reconnect-and-resume
- `youtube/events.rs` — `snippet.type` to `(kind, amount)`, the table above
- `youtube/oauth.rs` — the one-time consent flow, loopback redirect

The settings gain a YouTube section for the credentials and for
`event_words` entries prefixed `youtube:`. Those need no code beyond what
exists — `pricing_for("youtube")` and `priced_events()` already do the right
thing, and the Events section of the panel is meant to be built from
`priced_events()` rather than a hand-written list, so `youtube:superchat`
becomes priceable the moment that rendering lands.

## Order of work

1. **The credential prompt.** Nothing else can be verified without it, and
   Twitch is still blocked on the same thing. It is the highest-value
   unblocked-by-nothing item left.
2. **`youtube/model.rs` + `events.rs`,** with tests. Pure parsing, no network,
   testable immediately against fixture JSON — which is most of the risk, since
   the parsing is where a wrong field name silently prices a Super Chat as
   flat.
3. **`stream.rs`** — reconnect, resume from `nextPageToken`, no double count.
4. **`oauth.rs` + `auth.rs`**, then run against a real channel.
5. **Chat commands** via `insert`, on both platforms.
6. StreamElements tips, if wanted.

Steps 2 and 3 are the substance and need no credentials. Step 1 is what makes
steps 4 and 5 verifiable, and it is the thing standing between this project and
any end-to-end test of its own chat integrations.
