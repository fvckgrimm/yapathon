# yap-a-thon

A yapathon counter for OBS. Say words. Chat adds words. The run ends when you
reach the goal or hit the hard limit, whichever comes first.

GPUI desktop app, not an OBS plugin. The app serves two pages on loopback — a
control dock and an on-stream overlay — and OBS loads them as a Custom Dock and
a Browser Source. One Rust codebase covers Windows, macOS and Linux with nothing
OBS-specific to build, sign or maintain.

## How counting works

The one rule everything else follows from:

> **A word is counted only from a complete utterance.**

Audio is gated by voice activity detection. The segmenter buffers until silence,
transcribes that utterance once, counts its tokens, and discards the buffer. No
recogniser ever sees partial audio.

This matters because of what the alternative does. Feed a rolling window to
whisper and it re-reads overlapping audio, reporting the same sentence twice —
so the count inflates quietly, with no error anywhere. Counting interim results
from a streaming API has the same defect at segment boundaries. Both are avoided
by keeping utterance finalisation above the engine, so no engine can be asked to
transcribe something that might still change.

Two further guards against invented words, which would corrupt the score
directly:

- The segmenter's RMS energy floor rejects audio too quiet to be speech, before
  an engine sees it.
- Segments whisper itself reports as non-speech (`no_speech_probability`) are
  discarded.

## Layout

| Crate | Responsibility |
| ----- | -------------- |
| `yap-core` | Counting rules, goal math, session state, the engine trait |
| `yap-voice` | Microphone capture, VAD segmentation, whisper.cpp and Groq engines |
| `yap-events` | Twitch EventSub and chat commands behind a `ChatPlatform` trait |
| `yap-server` | Local HTTP + WebSocket API, and the two pages OBS loads |
| `yap-app` | GPUI window, and the wiring between all of the above |

`yap-core` has no audio, no network and no window, so every rule that decides
whether a count is fair is testable by feeding it text.

## Modes

Chosen per yapathon, from the **Run tab's mode buttons** — not from the settings
file. The goal and limit fields sit directly under them and re-label themselves
when the mode changes (`Goal (words)` becomes `Clock (seconds)`), because typing a
duration into a field labelled "words" is how a run ends up wrong by a factor of
a hundred and nobody notices until it is over.

- **Fixed** — the goal never moves. Events grant score only.
- **Extend** — the goal grows by each event's bonus. The classic chat-extends-the-
  goal run, and the one that needs a hard limit.
- **Time** — the goal is a duration and events extend the clock.

The hard limit is what makes an extend run terminate. Without it a busy chat
makes the run unbounded; with it, reaching the cap ends the run and says so.

Empty means "no limit", which is a legitimate choice. A goal that is empty or
zero falls back to 5,000 rather than starting a yapathon that is over before it
begins. `5,000` and `2k` are both accepted.

The app window is the place to run a yapathon from. It has the mode, goal, limit,
start/pause/resume/undo/end, and the correction tools. The dock page at `/dock`
has the same run controls as a fallback for when the window did not open or OBS
is on another machine; `/overlay` is only ever the on-stream card.

### When the count appears

Nothing is counted mid-sentence. Audio is buffered until a pause, and the whole
utterance is transcribed once — never a rolling window, which double-counts
every word that spans two windows, and never interim results, which double-count
at every segment boundary. That is a deliberate trade: the count is honest and
late rather than live and wrong.

How late is bounded by the model. On CPU with `small.en` an utterance takes a
couple of seconds; on a GPU it is close to instant. `whisper_backend_init_gpu:
no GPU found` in the log means the CPU path, which is expected and not an error —
`small.en` is a reasonable size for it. If you want faster, a smaller model or a
GPU backend both help; if you want more accurate, a larger model is the trade and
it will be slower still.

## Running

```sh
cargo run --release
```

A debug build is slow enough to matter here: whisper on CPU is the bottleneck.

Then point OBS at the URL the app prints:

```
yap-a-thon: dock at http://127.0.0.1:8973/dock
yap-a-thon: overlay at http://127.0.0.1:8973/overlay
```

The overlay URL is also in the app window at the bottom of the Run tab, and
**clicking it copies it** — no typing a loopback address by hand.

The dock is either the app's own window, a normal browser tab, or an OBS Custom
Dock (**View → Docks → Custom Browser Docks**, then add
`http://127.0.0.1:8973/dock`).

Closing the window does not stop the app or the server — see
[The server outlives the window](#the-server-outlives-the-window) below.

## Getting the overlay into OBS

This is the part that catches people, so it is worth being precise.

The window's Run tab has the URL in a chip at the bottom — **click it and it
copies**. Then in your scene: **Sources → + → Browser**.

| Setting | Value |
| ------- | ----- |
| URL | `http://127.0.0.1:8973/overlay` (click it in the app to copy) |
| Width | `480` (or your scene width) |
| Height | `270` (or your scene height) |

The overlay polls `/api/state` every two seconds as well as subscribing to the
WebSocket, because OBS's CEF blocks WebSockets to private-network addresses on
some configurations and a source whose only feed is the socket would then sit on
its initial state forever.

It starts visible even with no yapathon running — it says "no yapathon" rather
than showing nothing — so an empty source is always distinguishable from a broken
one.

### If the Browser Source shows nothing

Work down this list in order. Each step rules out a different cause, and the
first two are one screenshot each rather than a guess.

**1. Is the app running?**

```sh
curl -s http://127.0.0.1:8973/ping
```

That prints `ok`. If it hangs or refuses, the app is not running — start it and
watch for the two URL lines it prints.

**2. Can OBS reach the app at all?**

Put this URL in a Browser Source:

```
http://127.0.0.1:8973/ping
```

Just `ok` in tiny text, but visible. Plain text goes through no CSS engine, no
layout and no JavaScript, so if you see it, the network path is fine and the
problem is in the page. **If this shows nothing, stop reading this list** — it is
not a CSS problem:

- **Is the app still running?** Closing the dock window does *not* stop the server
  (that used to be a bug here), but quitting the app does.
- **Is the source in the scene you are streaming?** It has to be in the scene
  selected under *Scene* in the preview, and the preview has to be showing. A
  source can be perfectly correct and invisible if the scene is not active.
- **Is "Shutdown source when not visible" checked?** It is off by default. If it
  is on, OBS tears the page down whenever the source is not in the active scene,
  and a source whose scene is never active never loads at all.

**3. Does the page itself render?**

```
http://127.0.0.1:8973/overlay?debug=1
```

The whole canvas goes solid magenta with the URL, viewport size and whether the
API answered.

- **Magenta** → OBS is loading the page fine and something in the overlay's own
  styling does not render. Carry on to step 4.
- **Nothing** → OBS is not loading the page, which is step 2 again.

**4. Strip it back to what OBS definitely renders.**

```
http://127.0.0.1:8973/overlay?opaque=1
```

Same overlay, but the card's background is fully opaque instead of
`rgba(16,16,20,0.92)`. If this shows and the default does not, your GPU path is
having trouble compositing alpha, and this URL is the workaround.

**5. Check the two remaining silent causes.**

- **Width and height are zero.** A Browser Source defaults to a usable size, but
  one created from copied settings or resized to nothing has nothing to draw
  into. The card needs at least 16rem of width.
- **Stale cache.** CEF caches hard. The page is served `no-store`, but if you
  have changed the app and still see the old build, right-click the source →
  Refresh rather than adding a query string.

### A note on `backdrop-filter`

The card has no `backdrop-filter`, and that is deliberate. OBS's CEF does not
render it reliably — it can promote the element into a compositing layer that
comes back empty, taking the card with it, while the same page looks perfect in a
real browser. That is exactly the failure this overlay had: valid DOM, correct
layout, nothing on screen in OBS. The same goes for `filter`, `text-shadow` and
`mix-blend-mode`. There is a test that fails if any of them reappear.

### The server outlives the window

Closing the dock window leaves the app running so the overlay keeps working.
That is on purpose: the HTTP server OBS reads lives in this process, so quitting
on window-close blanked the overlay mid-stream for anyone who closed the window
to get it out of the way. Stop the app from the terminal or your window manager
when you are done.

## Looks

Both the window and the overlay are themeable, and neither requires anything to
be installed to try one.

**The overlay** — what viewers see. Five looks, set in Settings → Server, or
previewed one at a time by editing the URL:

```
http://127.0.0.1:8973/overlay?theme=minimal
```

| Theme | What it is |
| ----- | ---------- |
| `card` | the default: a dark rounded panel, bottom-right |
| `minimal` | square, borderless, quiet, top-left |
| `cute` | pastel and pill-shaped |
| `neon` | hard corners, one electric accent, a glow |
| `mono` | greyscale and small |

`?theme=` beats the setting, so comparing looks means editing a URL in a browser
and copying the winner into OBS. `?opaque=1` still works on every theme, for a
GPU path where the card's alpha composites to nothing.

**The URL the app offers you already has the theme in it** —
`http://127.0.0.1:8973/overlay?theme=neon`. That is deliberate: a Browser Source
is created once and then left alone, so a bare `/overlay` would mean changing the
theme in settings silently changed what is on stream. With the theme in the URL,
the source says which look it is.

**Changing the theme does not need the source re-added.** Every theme is already
in the served page, so the overlay swaps its own `data-theme` attribute on the
next poll — about two seconds, no reload, no flash. A URL with `?theme=` in it is
pinned, though: nothing the app says afterwards will override a look the streamer
chose explicitly.

**The window** — Settings → Server → Accent colour: teal, green, pink, purple,
amber, blue, or mono. Light and dark are a separate setting.

All of it applies the moment you press Save. The accent and the overlay theme
are pushed into the running app, and the overlay picks the change up on its next
poll — nothing needs a restart, and a setting that changes a file but nothing
else is worse than no setting at all.

Every colour in both places is checked against the surface it sits on at 4.5:1 or
better, in both light and dark — a test, not an opinion. That check is why the
pastel theme's muted text is a deep mauve rather than the pale pink it started
as, and why the dark theme's accent is lighter than the eye first reaches for.

## Settings

Three UIs, and two of them are not the window:

| What | Where |
| ---- | ----- |
| **App window** | The native GPUI window. Run tab and Settings tab. |
| **Dock page** | `http://127.0.0.1:8973/dock` — run controls only, no tabs. |
| **Overlay** | `http://127.0.0.1:8973/overlay` — the on-stream card. |

Settings live in the **Settings tab of the app window**. The dock page has no
Settings tab; it is run controls only.

The tab is schema-driven — every setting is described once in `settings.rs` — so
adding one costs a line rather than a label, an input and a save handler that can
drift apart. The Save button stays disabled until something actually changes, so
opening Settings and looking at it does not rewrite the file. Engine and model
changes take effect on the next launch, and the panel says so while a yapathon is
running rather than letting you change a limit and watch nothing happen.

Secrets are never fields. A token or API key goes in the OS credential store,
never rendered as a value and never written to `settings.toml` — that file is the
one thing on a streaming machine that gets synced, backed up and pasted into
support threads.

### Where the settings file is

Two places, checked in this order:

1. **Next to the executable** — `<dir of yap-a-thon>/settings.toml`
2. **The platform config directory** — `~/.config/yap-a-thon/settings.toml`

If neither exists the app writes the current defaults to
`~/.config/yap-a-thon/settings.toml` at startup, so there is a real file to open
and edit from the first launch rather than one that appears after your first
save. It never overwrites a file that is already there — including one that
failed to parse, because the startup warning about that file is the whole reason
it was kept.

The first one that exists is the settings, in full. They are never merged: a
value only the other file mentions comes from the defaults, so there is always
exactly one file in effect and it is the one the Settings tab's Save button
writes to. The panel prints the path it is using at the top.

The executable directory is checked first because a settings file sitting next to
a binary is a deliberate act — that is a portable install, where the binary and
its configuration travel together. If you install normally and keep your settings
in `~/.config`, the executable directory has no `settings.toml` and the second
candidate is used, which is the ordinary case.

Startup logs which path it chose and which it ignored, so "it ignored my settings
file" is never a mystery:

```
[INFO] settings: reading /home/you/.config/yap-a-thon/settings.toml
[INFO] settings: ignoring /opt/yap-a-thon/settings.toml while ... is in use
```

`settings.example.toml` in the repo root documents every key with its default
and says why the debatable ones are what they are. Copy it to whichever of the
two locations above you intend to use.

One caveat worth knowing: during development the executable lives in
`target/debug/`, so a `settings.toml` beside the release build is not the same
directory and will not be found. `cargo run` reads `target/debug/settings.toml`;
a built binary reads what sits beside it.

### What each event is worth

The **Events** section prices every kind of chat event in words. This is the
number chat moves the goal by, so it is worth setting deliberately rather than
leaving whatever the defaults happen to be.

| Setting | Default | Notes |
| ------- | ------- | ----- |
| New subscriber | 100 | one sub, one award |
| Resubscribe | 100 | one resub, one award — **not** multiplied by months |
| Gifted sub (each) | 50 | **per sub**, not per gift |
| Cheer (per bit) | 1 | 100 bits is a dollar, so 1 keeps a cheer meaningful |
| Raider | 250 | |
| Channel redemption | 100 | |
| Chat `!command` | 100 | |

The two per-unit rows are the ones that used to be wrong, and they are wrong in a
way that is only visible during a big event. Twitch sends a gift bomb as **one**
notification carrying a quantity, and the amount was never read off the payload —
so a hundred gifted subs extended the goal by exactly what one gifted sub did.
A cheer had the same problem. Both now scale, and there are tests that pin it.

Blanking a price means that event is worth nothing. It does not mean "leave it as
it was" — clearing the box is how you stop an event type from being able to move
the goal at all.

### Picking a microphone

The **Voice** section lists every input the machine can record from, read live
each time the panel draws, so a USB interface plugged in mid-session shows up
without a restart. **system default** is the first option and is stored as an
empty value, because "whatever the OS thinks" is the right answer for almost
everyone and should not be spelled as whichever device happens to be first.

Names are stored rather than indices, because indices are assigned by
enumeration order and a device being unplugged renumbers the rest.

If the list cannot be read, the panel says so instead of showing an empty list.
"No microphones" and "could not ask the machine" are different problems with
different fixes, and an empty list claims the first.

### Picking a model

The Engine section has two ways in, and neither requires typing an absolute path:

- **The list.** Every ggml model found in the searched directories, with its
  size, largest first. Click one, then Save.
- **`browse for a model file…`.** The desktop's own file chooser, filtered to
  `.bin` and `.gguf`, starting in whatever directory the current model lives in.
  This is the one to use for a model on another drive, in a downloads folder, or
  anywhere the search does not reach.

The chooser opens as its own window — on Linux it goes through the XDG desktop
portal, which is why the app needs no GTK libraries to build. The Browse button
greys out while it is open so you cannot stack two.

If you put a model somewhere neither of those covers, the app finds it anyway: a
configured path that does not exist is replaced with one found in
`~/.local/share/yap-a-thon/models/` or the current directory, and the dock says
which it picked. An existing path is never overridden, so pointing at a model on
another drive sticks.

Whichever way you choose it, the change needs a restart to take effect — the
model is loaded once at startup, and swapping it mid-yapathon would mean the
engine changed halfway through a count. The panel says so while a run is live.

## Setup

### A model

The default engine is whisper.cpp running on this machine — free, offline, and the
audio never leaves the box. Download a ggml model and put it in:

```
~/.local/share/yap-a-thon/models/
```

Then pick it in the Settings tab. Or write the path yourself:

```toml
[engine]
model_path = "/path/to/ggml-small.en.bin"
```

| Model | Size | Notes |
| ----- | ---- | ----- |
| `ggml-small.en.bin` | 466 MB | English only. Runs on CPU. Proves the pipeline works. |
| `ggml-medium.en.bin` | 1.5 GB | English only, better. Wants a GPU. |
| `ggml-large-v3-turbo.bin` | 1.6 GB | Multilingual, best. Wants a GPU. |

The `.en` models do English only; a multilingual model handles any language you
speak. Loading a model takes a few seconds, during which the counter sits at zero
— that is the model loading, not a fault.

### Compute backend

whisper.cpp selects its backend when the library is compiled, so each GPU backend
is an opt-in build feature. A CPU build works everywhere; GPU builds are faster:

```sh
cargo build --release --features yap-voice/whisper-cuda     # NVIDIA
cargo build --release --features yap-voice/whisper-metal    # Apple silicon
cargo build --release --features yap-voice/whisper-vulkan   # otherwise
```

### A cloud engine, instead

Optional. Groq's hosted `whisper-large-v3-turbo`, the same model, so counts stay
comparable. VAD still runs locally, so the API receives the same complete
utterances the local engine would and no word is counted twice at a streaming
boundary.

```toml
[engine]
kind = "groq"
credential = "yap-a-thon/groq"
```

The key goes in the OS credential store, never in `settings.toml` — that file gets
synced, backed up and pasted into support threads. About $0.04/hour of audio.

### Twitch

All three of these are needed; a partial config is treated as no config rather
than half-working, because EventSub refuses the socket without a client id and
silently receives nothing without a token.

1. Register an application at [dev.twitch.tv/console/apps](https://dev.twitch.tv/console/apps).
2. Get a user access token with the channel scopes.
3. Store the token in the OS credential store under the `credential` name above.
4. Set the channel login and the application id.

```toml
twitch_channel = "your_login"
twitch_client_id = "your_app_id"
twitch_credential = "yap-a-thon/twitch"
```

Without any of this, chat commands still work and a yapathon is still playable —
which is also the fastest way to try it.

### Chat commands

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

## When the count is wrong

Two levels, and it matters which one you reach for.

**Undo last** voids the most recent utterance in the log. It is the right answer
for "that was misheard" — one click, and the transcript shows the line struck
through rather than silently disappearing.

**Manual correction** is for everything else, and it has three shapes:

- **Preset buttons** — `+10 +25 +50 +100 −100`. Most corrections are round
  numbers, and there is a `−` as well as a `+` on purpose: a panel where every
  available correction goes up is a panel where a mistake can only be made
  worse.
- **A signed amount and a note.** Anything the presets do not cover, with the
  reason kept in the log.
- **Set the count to exactly this.** The case the other two cannot serve: a gift
  bomb extended the goal by an amount nobody agreed on and you know what it
  should have been. Type the number it should be and the difference is applied as
  a correction, with a note saying what it did.

All three go through the same audit log. Nothing is deleted, so "how did we get
here" always has an answer.

## Counting rules

Everyone will argue about these, so each is an independent switch. Three have a
defensible default:

- **Contractions expand.** "don't" is said as two words.
- **Numerals spell out.** The recogniser emits `42`; the streamer said "forty
  two", which is two words. Counting the digit as one makes number-heavy speech
  score far below how it sounds.
- **Repetitions count.** "no no no" is three words and the streamer meant all
  three.

Fillers are tallied separately and subtracted only if asked. Excluding them is a
variant mode rather than the honest default.

## The audit log

Every utterance's text and timestamps are kept, and a correction voids an entry
rather than deleting it. A yapathon is a public claim about how many words
someone said, and chat will check it. `Undo last` and a manual adjustment both
exist for the same reason: when the counter is wrong, the answer is a button, not
an argument.

## Status

Working: counting, all three goal modes and the hard limit, the audit log, undo
and the three correction shapes, chat commands, per-event pricing, the live
microphone picker, the model file chooser, click-to-copy overlay URL, the local
API, the dock, the overlay, and the settings panel.

Not yet: Twitch EventSub runs but has not been tested against a live channel;
there is no model download or first-run wizard; session history is not persisted
between runs; the settings panel's secret fields need a credential prompt, which
is not built, so a token still has to be set outside the app.

Verified by test rather than by eye:

- Every text colour clears WCAG AA (4.5:1) against every surface it is used on,
  in **both** themes. This is checked against the real palette in a real window,
  not against a table of literals — the OBS URL sat at about 1.5:1 until that
  check existed.
- Every exclusive-choice control — tabs, run modes, settings sections, choice
  fields, microphones — is clickable, tested by dispatching a real click at every
  option from every starting state. All four of those rows were once built with
  `disabled(!selected)`, which removes the click handler, so only the
  already-selected option responded and the entire settings panel was decorative.
- A counted session repaints the window. The session entity used to fold new
  snapshots into its model without notifying, so the count advanced in memory
  through an entire yapathon while the number on screen stayed at zero.

Not verified: word counting against real speech has not been measured end to end.
Every rule in `yap-core` is tested against text, and the segmenter's boundaries
are tested against synthetic audio, but the accuracy of the whole chain depends
on the model and on the room.

**The overlay has still not been seen rendering inside OBS**, though it turned
out to be OBS's own "disable hardware acceleration for browser sources" setting
rather than anything in the page. The `backdrop-filter` removal stands on its own
— it is a real CEF hazard either way.

**The settings panel's secret fields still need a credential prompt**, so a Twitch
token or Groq key has to be set outside the app. Everything else in the panel is
editable and persists.

Two bugs in this list were only findable by running it, and both had the same
shape — no error anywhere, just a symptom that looked like something else:

- The server and the window each built their own `AppState`, so they were two
  separate sessions. Starting a yapathon in one did nothing to the other.
- The listener called `stop()` on itself immediately after opening, with a
  comment saying it was holding it for the process lifetime.

Both are fixed and covered by tests. They are recorded here because "nothing
errored" is the hardest class of bug to chase, and the next one will probably
look like a rendering problem too.