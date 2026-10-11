# Running

```sh
cargo run --release
```

A debug build is slow enough to matter here: whisper on CPU is the bottleneck.

The app prints where it is serving:

```
yap-a-thon: dock at http://127.0.0.1:8973/dock
yap-a-thon: overlay at http://127.0.0.1:8973/overlay
```

The overlay URL is also in the app window at the bottom of the Run tab, and
**clicking it copies it** — no typing a loopback address by hand.

The dock is either the app's own window, a normal browser tab, or an OBS Custom
Dock (**View → Docks → Custom Browser Docks**, then add
`http://127.0.0.1:8973/dock`).

Both are loopback only. Nothing is exposed to your network, and nothing is
uploaded anywhere — see [Counting](counting.md) for what happens to your voice.

## Where the settings file is

Two places, checked in this order:

1. **Next to the executable** — `<dir of yap-a-thon>/settings.toml`
2. **The platform config directory** — `~/.config/yap-a-thon/settings.toml`, or
   `%APPDATA%\yap-a-thon\settings.toml`

If neither exists the app writes the current defaults to the second at startup, so
there is a real file to open and edit from the first launch. The path in use is
printed at startup and shown in Settings.

Portable installs win on purpose: a copy of the app that carries its own settings
is more useful than one that silently reads a machine-wide default. If a file
sits next to the executable but a config-directory file is also in use, the log
says so and names both.

## The server outlives the window

Closing the window quits the UI but not the app: the server keeps serving, and the
overlay in OBS keeps counting, because a yapathon should not die because someone
clicked the wrong thing. The microphone closes with the window — there is no
reason to keep recording when nobody is watching the window.

The port is held until the process actually exits, so if you relaunch quickly
you may see `Address already in use`. That is the old process still shutting
down.
