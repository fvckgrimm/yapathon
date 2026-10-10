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
SE_JWT=<token> node probe.mjs
# optionally, to be explicit about the channel:
SE_JWT=<token> SE_ROOM=<channelId> node probe.mjs
```

Needs Node 22+ for the built-in `WebSocket`. No packages, no install.

It writes `astro-<timestamp>.jsonl` — every frame, verbatim, with a local
timestamp — and prints a live line per event. Ctrl-C stops it and prints a
summary.

**3. Trigger events, then read the verdict.**

```sh
node analyse.mjs astro-<timestamp>.jsonl
```

The analyser collects the union of every key seen in each kind of payload and
checks it against what the pricing rule needs, so a field that only shows up on
the third event of its kind is not missed. It also reports median delivery
latency.

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
