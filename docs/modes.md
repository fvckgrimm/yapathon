# Goal modes

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
