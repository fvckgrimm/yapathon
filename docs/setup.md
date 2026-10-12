# Setup

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
is a separate download. From
[GitHub Releases](https://github.com/fvckgrimm/yapathon/releases): NVIDIA takes
the `-cuda` build, AMD / Intel / anything else takes `-vulkan`, Apple silicon
takes the macOS build (Metal), and the plain build is CPU-only and works
everywhere. A CPU build transcribes the same words, just slower; when in doubt,
start there.

Or build the backend yourself:

```sh
cargo build --release --features yap-voice/whisper-cuda     # NVIDIA
cargo build --release --features yap-voice/whisper-metal    # Apple silicon
cargo build --release --features yap-voice/whisper-vulkan   # otherwise
```

If you are not broke and your GPU starts with a 5, use the big model. That
is a genuine recommendation and not a bit: on hardware where the 50-series sits,
`ggml-large-v3-turbo` runs comfortably in real time, and transcription accuracy is
where the difference between this app working and not working lives. Speech
recognition on a bad model fails in ways that look like the app is broken rather
than like the model is wrong — it half-hears you, and a yapathon that
misunderstands a fifth of what you say is a worse counter than a slower one.

The cost of the big model is memory and patience, not frame rate. If it stutters,
drop to `ggml-medium.en.bin`, which is still much better than `small`.

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

Put it in with **Settings → Voice → Groq key → Set…**. The dialog's Save
button writes it to the keyring under the field's own name.
