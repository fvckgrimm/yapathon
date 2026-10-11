# Counting

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
