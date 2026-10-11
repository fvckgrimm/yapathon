# Astro probe

A throwaway tool for one question, asked before writing any integration code:

> **Do StreamElements' event payloads carry a size, so a Super Chat can be
> priced by what was actually spent and a cheer by its bits?**

That matters because `yap-core`'s entire pricing rule is "`amount` present means
scale by it, absent means flat". If every event through this gateway arrives
without a size, then every event would be a flat grant — a $50 Super Chat worth
the same as a $1 one — and the gateway would be a much poorer substitute for
native platform APIs than it first appears.

Nothing here is wired into the app. It connects, subscribes, dumps every frame
verbatim, and prints a verdict.

## Running it

**1. Get a JWT.** StreamElements dashboard → avatar (top right) → your channel →
copy the JWT token.

Two things that bite:

- Tokens are **account- and platform-specific**. With several accounts linked,
  copy it while switched to the account whose events you want.
- It is a secret. It goes in an environment variable, never in this directory,
  never in a commit.

**2. Run it while streaming.**

```sh
cp .env.example .env     # then fill it in
node probe.mjs
```

`.env` is gitignored. One variable per account, and the variable name becomes
the connection's label:

```
SE_JWT_YOUTUBE=<jwt copied while switched to the YouTube account>
SE_JWT_TWITCH=<jwt copied while switched to the Twitch account>
```

`SE_JWT=<token>` still works for a single account.

Every connection opens **at the same time**, not in sequence. StreamElements'
simulator only fires for whichever account is currently active, so testing them
one at a time proves each works and proves nothing about both at once — this
runs them concurrently, and the summary tells you whether each platform arrived
on exactly one connection, which is what the app needs to avoid counting a
cheer twice.

Needs Node 22+ for the built-in `WebSocket`. No packages, no install.

It writes `astro-<timestamp>.jsonl` — every frame, verbatim, tagged with which
connection delivered it — and prints a live line per event, prefixed with the
connection. Ctrl-C prints a summary.

**3. Trigger events, then read the verdict.**

```sh
node analyse.mjs astro-<timestamp>.jsonl
```

The analyser groups by connection first — room, providers seen, frame count —
then reports the union of every key seen in each kind of payload against what the
pricing rule needs, so a field that only shows up on the third event of its kind
is not missed. It also reports median delivery latency.

Two things it is looking for that a per-event table cannot show:

- **the same platform delivered by two connections**, which the app would count
  twice
- **a connection whose events are not the platform it is labelled with**, which
  means the token was copied while switched to a different account. The label is
  only a label — pricing follows each event's own `provider` — so this costs
  nothing but a confusing setup, and is worth knowing about.

## Checklist

Run the probe, *then* go and cause these. Most need you to be live.

| | How | Proves |
| --- | --- | --- |
| Twitch cheer | cheer bits in your own chat | is the **bit count** carried? |
| Twitch sub + gift sub | resub / gift subs | is the **gifted count** carried? |
| Twitch raid | raid yourself from a second account | is the **viewer count** carried? |
| YouTube Super Chat | buy one on your own stream | is the **money amount** carried? |
| YouTube membership | join your own channel | does `sponsor` fire, with an amount? |
| YouTube subscriber | subscribe a throwaway account | **does this fire at all?** The YouTube API has no such event |
| SE tip | tip your own channel via the tip page | does the amount survive? |
| Chat message | type in chat | shape of a `!yap` command |
| Tip from another person | ask someone, or use a second account | same, from a non-owner |

The YouTube subscriber row is the one to watch. It is the event the native
YouTube API cannot deliver at all, so if the gateway has it, the gateway is
doing something the native path fundamentally cannot.

Also worth timing: **leave it connected a week** and see whether the JWT still
works. A credential that rots is the same "worked until it quietly didn't"
failure this project has hit six times already.

## Two accounts at once

```
2 account(s): twitch, youtube
[twitch] connected  client=01M4M5DVXH913JQ2RXKG33GPY0
[youtube] connected  client=01M4M5DVXP9W1V6RMTAF33881X
[youtube] EVENT  youtube/superchat          CosmicNinja650    amount=5 [amount]
[twitch]  EVENT  twitch/cheer               Someone           amount=2500 [amount]

CONN       LABEL      ROOM                      PROVIDERS SEEN        EVENTS
1          twitch     5e7439a029f72805e1d577fb  twitch(18)            23
2          youtube    6acac21e4f4bb2e1b6a85f76  youtube(10)           15
```

Each connection resolves its own room from its own token, and each delivers its
own platform. Run it, switch the active account on StreamElements, simulate, and
confirm the other connection keeps its subscription alive throughout — that is
the property testing one account at a time cannot show.

## What the probe found (2026-10-10)

Answer: **yes, the amounts are there.** On `channel.activities`, nested one level
deeper than the topic page documents — in `data.data`:

| activity | `data.data` carries |
| --- | --- |
| `cheer` | `amount` = **bits** (1000, 5000), `message` |
| `raid` | `amount` = **viewers** |
| `tip` | `amount` = money. **No currency on this topic.** |
| `communityGiftPurchase` | `amount` = **count of gifted subs** |
| `subscriber` | `amount` = **months**, `tier`, plus `gifted`/`sender` |
| `follow` | nothing — correct, a follow is flat |

`channel.session.update` carries the same values under Session Data Reference key
names. It is redundant with `activities` for our purposes, except that it is the
topic the docs are actually accurate about.

**Three corrections to StreamElements' own documentation**, all found by this:

- The `channel.activities` topic page lists only `username`, `displayName`,
  `providerId` and `avatar`. It omits `amount`, `tier`, `gifted`, `sender` and
  `message`, which are all present.
- The `channel.session.update` page says `data.key` and `data.name` are "the same
  value". A real payload carries **only `name`**. Reading `data.key` labels every
  session frame `undefined`.
- `channel.activities` does not document an `activityGroup`, which exists and is
  how gift-bomb members are grouped, nor `activityId`/`_id`, which are the
  dedup keys.

### YouTube, measured the same way

A **separate JWT is required per platform account**, and it changes the room:

| run | room | provider seen |
| --- | --- | --- |
| Twitch JWT | `5e7439a0…` | twitch |
| YouTube JWT | `6acac21e…` | youtube |

The probe subscribed with no room and the server returned the canonical one, so
this was discovered rather than configured. Two accounts means two JWTs, which
means either two connections or one connection carrying two subscriptions with
different tokens — worth testing, since the subscribe frame carries its own
`data.token`.

| activity | `data.data` carries |
| --- | --- |
| `superchat` | `amount` = money |
| `sponsor` | `amount` — a channel **membership** |
| `subscriber` | nothing — a channel **subscriber** |
| `tip` | `amount` |
| `merch` | `amount`, `price`, `quantity`, `items` |

**`subscriber/youtube` fires.** That is the whole ballgame: the native YouTube
Live Streaming API has no follow notification anywhere, so this is the one event
an Astro-connected channel gets that a native YouTube one never can. Note that
`sponsor` (membership) and `subscriber` are distinct here, which is exactly the
distinction the native API makes too — for memberships.

### Still unproven

- **Every event in every dump is `isMock: true`.** All four runs used the
  widget simulator. The schema is confirmed for both platforms; real traffic is
  not.
- **No currency, anywhere.** `currency` appears zero times across all 60 frames,
  on both platforms, on both topics. `superchat.amount` is a bare number, so
  €5 and $5 would price identically. A cross-streaming streamer who prices
  "per dollar" would get this wrong for anyone not paying in dollars.
- **`channel.chat.message` never fired** — zero events in all four runs. Chat
  commands are how `!yap` works, so this needs confirming with a real stream;
  the simulator may simply not send chat.
- **The JWT's lifetime is unknown.** No run has lasted long enough to find out.

## Notes

- Only **public** YouTube livestreams are supported by StreamElements.
- `channel.session.update` is the topic most likely to carry the amounts —
  its `data.key` names are exactly the Session Data Reference keys, and that
  reference documents `cheer-latest.amount` (bits), `superchat-latest.amount`,
  `raid-latest.amount` (viewers) and `subscriber-gifted-latest.amount` (count).
  The `channel.activities` topic's own page lists only username/displayName/
  providerId/avatar, which is either an incomplete page or a genuinely thinner
  payload. That is the specific thing this probe is here to find out.
- `room` is optional; omitting it subscribes to the authenticated channel.

## Dumps contain real people's usernames

`*.jsonl` is gitignored. Do not paste one into a bug report or commit it.
