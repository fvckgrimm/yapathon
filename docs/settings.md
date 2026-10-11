# Settings

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

**Setting one is a button.** Next to each credential the panel shows whether it
is set, and a **Set…** button opens a masked input with a reveal toggle. The
dialog's own **Save** button is what commits it, to GNOME Keyring, KWallet, the
macOS Keychain or the Windows Credential Manager, under the field's name.

There is deliberately **no file fallback**. On a machine with no credential store
the panel says so and offers no button, rather than quietly writing the key to a
dotfile. The moment that matters is a machine whose store is missing, which is
exactly the machine nobody is thinking about later — so it fails loudly instead.

The Save button at the bottom of the panel is **not** how a credential is
committed, and typing one does not enable it. That is structural rather than a
promise: the credential inputs have no change subscription, so they never reach
the model that Save serialises.

### What each event is worth

The **Events** section prices every kind of chat event in words. This is the
number chat moves the goal by, so it is worth setting deliberately rather than
leaving whatever the defaults happen to be.

Prices are keyed `<platform>:<kind>`, so `twitch:cheer` means the same thing
whether it arrived through Twitch's own EventSub or through StreamElements, and
switching source does not re-price anything. The panel lists **twitch**,
**youtube** and **kick**.

| Setting | Default | Notes |
| ------- | ------- | ----- |
| New subscriber | 100 | one sub, one award |
| Resubscribe | 100 | one resub, one award — **not** multiplied by months |
| Gifted sub (each) | 50 | **per sub**, not per gift |
| Cheer (per bit) | 1 | 100 bits is a dollar, so 1 keeps a cheer meaningful |
| Raider | 250 | per raider |
| Channel redemption | 100 | |
| Chat `!command` | 100 | |
| Follower, host, tip, merch | *(blank)* | |
| YouTube subscriber | *(blank)* | only StreamElements can report this at all |
| YouTube membership | *(blank)* | a paid member joining — not the same as subscribing |
| YouTube Super Chat | *(blank)* | per unit spent; **no currency arrives with it** |
| Kick follower, sub, KICKs, tip | *(blank)* | |

The rows left blank are blank on purpose. They are the kinds the app can receive
but nothing has a sensible default for, and a Super Chat priced at an invented
rate is worse than an unpriced one you fill in deliberately. Unset them and the
app says so once rather than letting the goal quietly fail to move.

The two per-unit rows that *are* set used to be wrong, and they were wrong in a
way only visible during a big event. Twitch sends a gift bomb as **one**
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

Where to *get* a model is in [Setup](setup.md). This is choosing one you already
have.

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
