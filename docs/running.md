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

## If the credential store is locked

The app will tell you `locked — unlock it, then check again`, and the log will say
so once. It does this instead of hanging, which is what it used to do.

A **locked** keyring is not a missing one, and only one of them is fixable:

| | Meaning | Fix |
| --- | --- | --- |
| **locked** | the store is there, and waiting for an unlock prompt | unlock it |
| **no credential store** | nothing to talk to on this machine | nothing — the app will not keep your keys anywhere else |

The difference matters because they look identical from inside the app, and the
fix for one is a command while the fix for the other is nothing.

### Linux

Only the Secret Service implementations really lock, so this is overwhelmingly a
GNOME problem. **KWallet normally unlocks with your session** and the problem
does not arise.

```sh
gnome-keyring-daemon --unlock      # prompts for your login password
```

Or log out and back in, which unlocks it for the session. `seahorse` gives you a
GUI if you would rather click than type a password into a terminal.

A desktop that has been idle long enough may lock it again; that is the keyring's
own timeout, not this app.

### macOS

The login Keychain does not lock the way a Secret Service one does — it normally
prompts on first use. Reaching this state usually means a non-interactive
session rather than a locked keychain.

### Windows

Windows Credential Manager does not lock at all, so this state means no store is
reachable, and there is nothing to unlock.

### The app will not ask for your login password

Deliberately, on every platform. It is your *OS* keyring password, not a
yap-a-thon credential, and a box labelled "Groq API key" that silently wanted the
other one would be a genuinely nasty surprise. So the app explains and you decide.
