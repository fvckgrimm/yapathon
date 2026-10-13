# yap-a-thon

A yapathon counter for OBS. Say words. Chat adds words. The run ends when you
reach the goal or hit the hard limit, whichever comes first. Subathon for Yappers.

GPUI desktop app, not an OBS plugin. The app serves two pages on loopback — a
control dock and an on-stream overlay — and OBS loads them as a Custom Dock and
a Browser Source. One Rust codebase covers Windows, macOS and Linux with nothing
OBS-specific to build, sign or maintain.

## Why this exists

My fucking guys [speedy](https://www.youtube.com/@KYRSP33DY) and
[side](https://www.youtube.com/@SideArms4Reason) kept joking about running a
yapathon on stream. I wanted to find out whether one was actually possible, and
had no experience building anything for streams beyond archiving them.

I am a viewer, not a collaborator, and they have not asked for this.

## Built with AI

Written with Claude, an LLM assistant, throughout — including the tests. Roughly
everything in this repository was produced with AI assistance rather than typed.

Worth saying plainly rather than leaving it to be discovered: AI-written code
carries the failure modes AI code carries, and this project's is unusually
consistent. There were seven bugs where a value was computed correctly and then
silently discarded — a button wired to nothing, a repaint never requested, a
price read into a map that nothing then looked up. None of them threw an error.
None of them were visible from the screen. That is worth knowing before you trust
any of it, and it is why there are as many tests as there are.

## Quickstart

**1. Download it.** Grab the zip for your machine from
[GitHub Releases](https://github.com/fvckgrimm/yapathon/releases) — no
build needed. Pick by OS, then GPU:

| Machine | Download |
| --- | --- |
| Windows + NVIDIA GPU | `yap-a-thon-windows-x86_64-cuda.zip` |
| Windows + AMD / Intel / other GPU | `yap-a-thon-windows-x86_64-vulkan.zip` |
| Windows, no GPU or unsure | `yap-a-thon-windows-x86_64.zip` (CPU, works everywhere, slower) |
| Linux + NVIDIA GPU | `yap-a-thon-linux-x86_64-cuda.zip` |
| Linux + AMD / Intel / other GPU | `yap-a-thon-linux-x86_64-vulkan.zip` |
| Linux, no GPU or unsure | `yap-a-thon-linux-x86_64.zip` (CPU, works everywhere, slower) |
| macOS, Apple silicon | `yap-a-thon-macos-aarch64.zip` (Metal) |

CUDA builds want a current NVIDIA driver; Vulkan builds want a GPU driver
with Vulkan support (on Linux that is usually already there with Mesa). When
in doubt take the plain CPU zip — same app, just slower transcription. See
[Setup](docs/setup.md#compute-backend) for what the backends mean.

Or build it yourself:

```sh
git clone <this repo> && cd yap-a-thon
cargo build --release
./target/release/yap-a-thon
```

**2. Download a speech model.** The app needs one, and does not fetch it for you
— a 500 MB download on first launch is a decision, not a surprise.

Models live in the
[whisper.cpp repository on Hugging Face](https://huggingface.co/ggerganov/whisper.cpp/tree/main).
Grab `ggml-small.en.bin` as a starting point; `ggml-base.en.bin` is faster and
`ggml-medium.en.bin` is more accurate if you have a GPU. If you are not broke and
your GPU starts with a 5, go straight to `ggml-large-v3-turbo.bin` — see
[Setup](docs/setup.md#compute-backend).

This is a **local** model. It runs on your own machine, nothing you say is
uploaded anywhere, and it works with your connection off. The `.en` models are
English-only; the larger non-`.en` files handle other languages and cost more to
run.

Put it wherever you like, then point the app at it in **Settings → Voice →
Voice model** (or just open **Setup** — it lists every model found, no typing
needed). It will also look in `~/.local/share/yap-a-thon/models/` and
`%APPDATA%\yap-a-thon\models\` without being told to.

**3. Talk.** Press **Start**. Words appear.

That is a working yapathon with no chat integration. Everything below is about
letting chat move the goal too.

## Chat, if you want it

**Twitch** — register an application at
[dev.twitch.tv/console/apps](https://dev.twitch.tv/console/apps), set your
channel and application id in **Settings → Chat setup**, and put a user token with
the channel scopes in **Settings → Chat setup → Twitch login**.

**YouTube, and Twitch and YouTube at once** — the easier path, through
[StreamElements](https://streamelements.com). No OAuth project, no quota, and it
reports one event the YouTube API cannot report at all.

1. Sign in at **streamelements.com**.
2. **Click the avatar in the top-right corner**, then your channel name, then
   the correct account. Copy the JWT token.
3. In the app, go to **Settings → Chat setup → YouTube connection → Set…**, and paste it.
4. **Click the avatar again** and switch to your *other* account.
5. Copy that token too, into **Twitch via StreamElements**.

The tokens are per account — each only ever sees its own platform, and copying
the wrong one connects to the other account's room. If it is not working, the
app log says which platform each connection is actually receiving.

Tokens go in the **OS credential store** — GNOME Keyring, KWallet, the macOS
Keychain, Windows Credential Manager — never into `settings.toml`, which is the
file that gets synced and pasted into support threads. If that store is locked
the app says so and explains how to unlock it on your platform, rather than
hanging; see [Running](docs/running.md#if-the-credential-store-is-locked).

Then price your events in **Settings → Rewards** (or hit a preset: **Chill** /
**Hype**). Nothing is priced until you price it, and the app tells you when an
event arrives that you have not priced.

## Documentation

| | |
| --- | --- |
| [Running](docs/running.md) | building it, the URLs, where settings live |
| [Counting](docs/counting.md) | how words are counted, fillers, contractions, the audit log |
| [Goal modes](docs/modes.md) | extend, fixed and clock modes, and what chat can do to each |
| [Setup](docs/setup.md) | models, compute backends, cloud engines |
| [Twitch](docs/twitch.md) | EventSub, tokens, what it counts |
| [StreamElements Astro](docs/astro.md) | the third platform, and the probe used to measure it |
| [YouTube](docs/youtube.md) | why it works this way, and the quota that shapes it |
| [Overlay](docs/overlay.md) | themes, OBS setup, and the things CEF renders badly |
| [Settings](docs/settings.md) | every section, and the events you can price |
| [Troubleshooting](docs/troubleshooting.md) | when the count is wrong, or nothing happens |

## What it does not do

No events have come from a real viewer yet — everything verified so far used
StreamElements' own simulator, and the app labels those `(mock)` so they cannot
be mistaken for the real thing. Super Chat amounts arrive without a currency, so
a price is per unit rather than per dollar. `!yap` in chat works through the
local API but not yet through StreamElements.

See [Status](docs/troubleshooting.md#status) for the rest.
