# Troubleshooting

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

Working: counting, all three goal modes and the hard limit, the audit log, undo
and the three correction shapes, chat commands, per-event pricing across twitch,
youtube and kick, the live microphone picker, the model file chooser,
click-to-copy overlay URL, the local API, the dock, the overlay, the settings
panel, the in-app credential prompt, and StreamElements Astro.

Verified end to end: a yapathon counted live speech while its goal was extended
by chat from both platforms at once, with per-bit cheers and per-unit Super Chats
scaling correctly.

Not yet: no event has come from a real viewer rather than the simulator; there is
no model download or first-run wizard; session history is not persisted between
runs; native YouTube is not built, though Astro covers the ground
([`docs/youtube.md`](youtube.md)).

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

Not verified: word counting against real speech has been confirmed once — words
appeared and words-per-minute followed when someone talked — but accuracy has not
been measured properly. Every rule in `yap-core` is tested against text, and the
segmenter's boundaries are tested against synthetic audio, so what is unmeasured
is the chain's behaviour on a real room and a real model.

**The overlay has still not been seen rendering inside OBS**, though it turned
out to be OBS's own "disable hardware acceleration for browser sources" setting
rather than anything in the page. The `backdrop-filter` removal stands on its own
— it is a real CEF hazard either way.

**The credential dialog has not been clicked through.** Its store round trip, its
masking, its empty-value refusal and the fact that typing into it does not arm
Save are all tested, and the panel renders it. But opening the dialog itself is
not covered headlessly — GPUI's test window has no component root, and building
one needs an `&mut App` that the test context does not hand out. So "click Set,
a dialog appears, paste a token, click Save, it says stored" is the one step
that wants a human.

Two bugs in this list were only findable by running it, and both had the same
shape — no error anywhere, just a symptom that looked like something else:

- The server and the window each built their own `AppState`, so they were two
  separate sessions. Starting a yapathon in one did nothing to the other.
- The listener called `stop()` on itself immediately after opening, with a
  comment saying it was holding it for the process lifetime.

Both are fixed and covered by tests. They are recorded here because "nothing
errored" is the hardest class of bug to chase, and the next one will probably
look like a rendering problem too.
