# StreamElements Astro

A third option, **off by default**, alongside Twitch and YouTube rather than
instead of either.

[StreamElements' Astro gateway](https://docs.streamelements.com/websockets) is a
WebSocket carrying twitch, youtube and kick on one connection, authenticated by
a token copied from their dashboard. No OAuth, no Cloud project, no quota. It is
a socket from this app, not a hosted overlay — nothing goes in OBS.

It is genuinely good at one thing neither native API can do: it reports
**YouTube channel subscribers**, which the Live Streaming API has no notification
for anywhere. Native YouTube gives memberships, Super Chats and member messages;
it cannot tell you who pressed Subscribe.

Settings → Astro, one field per platform: **YouTube account** and **Twitch
account**. Copy each JWT from the StreamElements dashboard while switched to that
account, and leave the other empty. Each is its own keyring entry, so the panel
says whether each is set and clearing one leaves the other alone.

The label is only organisation. A token does not say which platform it covers
and nothing checks, so a token saved in the wrong slot still works — every event
carries its own `provider`, which is what decides how it is priced. The log
names the platform that actually arrived, so a mistake is visible rather than
silent.

Prices are shared with the native path: Astro names events `<platform>:<kind>`
the same way EventSub does, so `twitch:cheer` means the same thing either way
and switching does not silently re-price everything to nothing.

Two things it does not do, deliberately:

- **No simulated events.** StreamElements' widget simulator marks its test
  activity `isMock: true`, and counting it would let a cheer nobody sent move the
  goal.
- **No duplicate counting.** Activities are deduplicated by `activityId`, so a
  reconnect cannot count one cheer twice.

**Astro and native Twitch cannot both count Twitch.** Both report
`twitch:cheer` with the same bits into the same sink, so a streamer who left both
configured would watch the goal climb at double speed with nothing wrong on
screen. Where a platform is covered by both, native wins and Astro says so in the
log and as a warning in the window — clear the native Twitch settings to hand
that platform to Astro instead. Astro is still worth adding for the platforms
native cannot cover.

### Testing it without viewers

```sh
YAP_ALLOW_MOCK_EVENTS=1 cargo run --release
```

StreamElements' widget simulator produces test events. Counting those would let a
cheer nobody sent move the goal, so they are normally dropped — this counts them
anyway, for checking the whole chain end to end without waiting for a raid.

Anything it accepts is labelled `(mock)` in the event feed, so it can never be
mistaken for something a viewer did. It is an environment variable rather than a
setting because `settings.toml` is exactly the file that gets synced to a
streaming machine and pasted into support threads, and it warns at launch.

**Unset it before streaming.**

The payloads were measured against the widget simulator; the connection and the
full chain — payload to priced event to a goal that moves — have been verified
against it. See [`tools/astro-probe`](../tools/astro-probe/README.md).

Work with no credentials at all:

```
!yap                 status
!yap start 5000      begin a yapathon
!yap extend 500      add 500 words to the goal
!yap +500            same, the form chat actually types
!yap set 7500        move the goal to an absolute value
!yap pause           stop counting without ending the run
!yap resume
!yap undo            void the last utterance
!yap end
```
