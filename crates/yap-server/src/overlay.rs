//! The two HTML pages OBS loads.
//!
//! Both are served from memory as one string each. A browser source pointing at
//! a `file://` path cannot reach the local server, and a streamer who edits a
//! file to change a colour is editing a file inside an app installation. One
//! served page is one thing to keep working.

use axum::extract::State;
use axum::http::header;
use axum::response::{Html, IntoResponse, Response};
use std::sync::Arc;

use crate::AppState;

/// The on-stream overlay.
///
/// Read by OBS as a Browser Source. Deliberately spare: a big number, what is
/// left to say, and the words chat just added. Anything more competes with the
/// streamer for attention, which is the one thing an overlay on their own
/// channel must not do.
const OVERLAY_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>yap-a-thon overlay</title>
<style>
  :root {
    /* Every themeable value is a variable so a theme is data, not a fork of
       this stylesheet. The defaults below are the `card` theme; the rules at
       the bottom override them by setting `data-theme` on the body. */
    --card: rgba(16, 16, 20, 0.92);
    --card-solid: #101014;
    --card-border: 1px solid rgba(255, 255, 255, 0.08);
    --fg: #f4f4f5;
    --dim: #a1a1aa;
    --accent: #4ade80;
    --warn: #fbbf24;
    --capped: #f87171;
    --radius: 0.75rem;
    --pad-y: 1.25rem;
    --pad-x: 1.5rem;
    --min-width: 16rem;
    --count-size: 3.5rem;
    --count-weight: 700;
    --count-spacing: normal;
    --bar-height: 0.375rem;
    --bar-radius: 999px;
    --track: rgba(255, 255, 255, 0.1);
    --corner-x: 2rem;
    --corner-y: 2rem;
    --glow: none;
    --font: ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
    /* The style defaults live here too, matching the `card` palette above, so
       a page with neither attribute set still renders the corner look. Every
       palette rule and every style rule repeats its own half of this, which is
       what lets the two axes be set independently. */
    --pad-y: 1.25rem;
    --pad-x: 1.5rem;
    --min-width: 16rem;
    --count-size: 3.5rem;
    --count-weight: 700;
    --count-spacing: normal;
    --corner-x: 2rem;
    --corner-y: 2rem;
  }
  * { box-sizing: border-box; }
  html, body {
    margin: 0;
    /* Explicitly transparent and full-bleed: OBS renders a browser source over
       whatever is behind it in the scene, and an opaque body hides the stream.
       Both the colour and the size have to be set — a body that sizes to its
       content leaves the rest of the source canvas unpainted. */
    background: transparent;
    width: 100%;
    height: 100%;
    overflow: hidden;
    font-family: ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
    color: var(--fg);
    /* The overlay sits over the stream, so it must not eat clicks meant for
       anything behind it. */
    pointer-events: none;
  }
  .card {
    position: absolute;
    right: var(--corner-x);
    bottom: var(--corner-y);
    /* `min-width` alone let the card overflow a narrow browser source and be
       clipped by `overflow: hidden`, which is how an overlay that works in a
       browser ends up invisible in a scene sized 320x180. `max-width` as well
       keeps it inside any source the streamer sizes. */
    min-width: var(--min-width);
    max-width: min(28rem, calc(100% - 4rem));
    box-sizing: border-box;
    padding: var(--pad-y) var(--pad-x);
    /* Opaque, and no `backdrop-filter`.
       OBS's CEF does not render `backdrop-filter` reliably — it can promote the
       element into a compositing layer that comes back empty, taking the card
       with it, while the same page looks perfect in a real browser. That is
       precisely the failure this file had: correct DOM, nothing on screen in
       OBS. A frosted look is not worth a blank overlay, so there is no blur
       here at all. */
    background: var(--card);
    border-radius: var(--radius);
    border: var(--card-border);
  }
  .status {
    font-size: 0.75rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--dim);
  }
  .count {
    font-size: var(--count-size);
    font-weight: var(--count-weight);
    letter-spacing: var(--count-spacing);
    font-variant-numeric: tabular-nums;
    line-height: 1.05;
    /* `box-shadow` and not `text-shadow` or `filter`: those promote the element
       into a compositing layer CEF can return empty, which is the failure that
       cost this overlay an entire debugging session. A box-shadow paints with
       the element and is safe. */
    box-shadow: var(--glow);
  }
  .target { color: var(--dim); font-weight: 400; font-size: 0.45em; }
  .bar {
    height: var(--bar-height);
    background: var(--track);
    border-radius: var(--bar-radius);
    overflow: hidden;
    margin: 0.75rem 0 0.5rem;
  }
  .fill {
    height: 100%;
    /* Full width, scaled from the left, so filling is a compositor transform
       rather than a layout. Transitioning `width` here relayouts the card on
       every counted utterance and every event — which during a stream is
       several times a second, for motion nobody asked for. */
    width: 100%;
    transform: scaleX(0);
    transform-origin: left center;
    background: var(--accent);
    /* 220ms on a real ease-out rather than the built-in `ease-out`, which is too
       weak to read as motion: it leaves and arrives without conviction. Long
       enough to see a sub push the bar backwards, short enough that the bar is
       never behind the number. */
    transition: transform 220ms cubic-bezier(0.23, 1, 0.32, 1);
  }
  @media (prefers-reduced-motion: reduce) {
    /* Someone who has asked for less motion gets a bar that jumps. The colour
       still carries the meaning; only the movement goes. */
    .fill { transition-duration: 1ms; }
  }
  .row {
    display: flex;
    justify-content: space-between;
    font-size: 0.8rem;
    color: var(--dim);
    font-variant-numeric: tabular-nums;
  }
  .event {
    margin-top: 0.5rem;
    font-size: 0.85rem;
    color: var(--accent);
  }
  .note {
    margin-top: 0.4rem;
    font-size: 0.75rem;
    color: var(--capped);
  }
  .capped .count { color: var(--capped); }
  .capped .fill { background: var(--capped); }
  .won .count { color: var(--accent); }
  .paused .count { color: var(--warn); }
  .hidden { display: none; }

  /* --- Themes -----------------------------------------------------------
     One rule per theme, overriding only what differs. Kept in the stylesheet
     rather than generated so the served page is still one static string, which
     is what lets `?debug=1` show the real thing and what keeps the CEF hazard
     list checkable by reading this file.

     A theme may not introduce `filter`, `text-shadow`, `backdrop-filter`,
     `mix-blend-mode` or `clip-path`. See the note above; there is a test. */

  /* --- Palettes: colour, and nothing else ---------------------------------

     Each rule below sets colours, a font, and the *character* of the shape —
     rounded or square, a glow, a pill bar. It never sets where the card sits,
     how big the number is, or how much padding there is. That is what lets a
     palette be paired with any style: a slim catppuccin overlay exists because
     the two axes do not know about each other.

     A palette may not set `--pad-y`, `--count-size`, `--min-width`,
     `--corner-x` or `--corner-y`. Those belong to the style, and a palette that
     set one would silently defeat it — there is a test. */

  /* Card: the default, spelled out rather than left to the `:root` values.
     Repeating them costs nothing and means every palette has a rule of its own —
     which is what lets a test check that the stylesheet and the Rust table
     agree, rather than one of them drifting silently. */
  body[data-theme="card"] {
    --card: rgba(16, 16, 20, 0.92);
    --card-solid: #101014;
    --card-border: 1px solid rgba(255, 255, 255, 0.08);
    --fg: #f4f4f5;
    --dim: #a1a1aa;
    --accent: #4ade80;
    --warn: #fbbf24;
    --capped: #f87171;
    --radius: 0.75rem;
  }

  /* Minimal: near-black, cool blue, square corners and no border. The card
     keeps a solid backing — white text over video with no plate behind it is
     unreadable a good fraction of the time, and legibility is the one thing a
     minimal look is not allowed to trade away. */
  body[data-theme="minimal"] {
    --card: rgba(8, 8, 12, 0.86);
    --card-solid: #08080c;
    --card-border: none;
    --radius: 0.25rem;
    --fg: #fafafa;
    --dim: #8b8b94;
    --accent: #7dd3fc;
    --warn: #fbbf24;
    --capped: #f87171;
    --track: rgba(255, 255, 255, 0.14);
  }

  /* Cute: pastel, pill-shaped, deliberately soft. The lightest palette, which
     means it is the one most at risk over a bright stream — hence a card that
     is nearly opaque. The accent is a deep rose rather than a pale pink
     because a pale pink on a pale pink card is decoration, not a number. */
  body[data-theme="cute"] {
    --card: rgba(255, 245, 251, 0.95);
    --card-solid: #fff5fb;
    --card-border: 2px solid rgba(196, 65, 127, 0.35);
    --radius: 1.75rem;
    --fg: #4a2c47;
    --dim: #75506f;
    --accent: #c2417f;
    --warn: #b45309;
    --capped: #be123c;
    --track: rgba(196, 65, 127, 0.18);
    --bar-radius: 999px;
    --font: ui-rounded, "Hiragino Maru Gothic ProN", Quicksand, "Segoe UI",
      ui-sans-serif, system-ui, sans-serif;
  }

  /* Neon: near-black, hard corners, one electric accent and a glow. The glow is
     a `box-shadow` rather than a `text-shadow` on purpose — see the note on
     `.count`. This is the palette most likely to be paired with a dark, neon
     scene, where it disappears into it by design. */
  body[data-theme="neon"] {
    --card: rgba(4, 4, 10, 0.94);
    --card-solid: #04040a;
    --card-border: 1px solid rgba(0, 229, 255, 0.45);
    --radius: 0.125rem;
    --fg: #f0f6ff;
    --dim: #7c8aa5;
    --accent: #00e5ff;
    --warn: #ffd166;
    --capped: #ff4d6d;
    --track: rgba(0, 229, 255, 0.16);
    --bar-radius: 0.125rem;
    --glow: 0 0 24px rgba(0, 229, 255, 0.35);
    --font: "JetBrains Mono", ui-monospace, "Cascadia Mono", Menlo, monospace;
  }

  /* Mono: greyscale, for a channel whose brand is already loud. Every colour is
     a grey, including the accent — the bar still fills, it just does not draw
     attention to itself doing it. */
  body[data-theme="mono"] {
    --card: rgba(17, 17, 17, 0.9);
    --card-solid: #111111;
    --card-border: 1px solid rgba(255, 255, 255, 0.14);
    --radius: 0.5rem;
    --fg: #ededed;
    --dim: #9a9a9a;
    --accent: #ededed;
    --warn: #c8c8c8;
    --capped: #ffffff;
    --track: rgba(255, 255, 255, 0.16);
    --bar-radius: 0.25rem;
    --font: ui-monospace, "JetBrains Mono", Menlo, Consolas, monospace;
  }

  /* Catppuccin Mocha: the most-used of the four catppuccin flavours, and the
     one people mean when they say catppuccin. Pastel over a warm dark base, with
     the mauve as the accent because green on mauve is the pairing the theme is
     known for. `mauve` and `overlay2` are its own named colours — taken from
     the published palette rather than eyeballed, so it matches what someone
     expects from the name. */
  body[data-theme="catppuccin-mocha"] {
    --card: rgba(30, 30, 46, 0.92);
    --card-solid: #1e1e2e;
    --card-border: 1px solid rgba(166, 173, 200, 0.22);
    --radius: 0.75rem;
    --fg: #cdd6f4;
    --dim: #a6adc8;
    --accent: #cba6f7;
    --warn: #f9e2af;
    --capped: #f38ba8;
    --track: rgba(166, 173, 200, 0.18);
    --font: ui-rounded, "Comic Sans MS", "Segoe UI", ui-sans-serif, system-ui,
      sans-serif;
  }

  /* Catppuccin Macchiato: cooler and lower-contrast than mocha — which is the
     honest reason to include it rather than the other two. Its darkest text on
     its own base is nearer 4.5:1 than mocha's, and its palette is the one that
     will fail the legibility check first if it is ever nudged lighter. Frappe
     and latte are not included for the same reason: both are pale, and a pale
     card over a bright stream is the failure this whole check exists for. */
  body[data-theme="catppuccin-macchiato"] {
    --card: rgba(36, 36, 54, 0.92);
    --card-solid: #242436;
    --card-border: 1px solid rgba(165, 173, 199, 0.22);
    --radius: 0.75rem;
    --fg: #cad3f5;
    --dim: #a5adc7;
    --accent: #c6a0f6;
    --warn: #eed49f;
    --capped: #f38fa8;
    --track: rgba(165, 173, 199, 0.2);
  }

  /* --- Styles: shape, and nothing else ------------------------------------

     Each of these decides where the card sits, how big the number is, how much
     padding there is, and which parts are drawn. None of them sets a colour.

     These live in `body[data-style="…"]` rules rather than beside the palettes
     above, and that placement is load-bearing: the base `.card` rule sets
     `right` and `bottom` directly, and two selectors of equal specificity are
     settled by source order. Position declared in the palette block — earlier in
     the file — would lose to the base rule and the card would stay hard against
     the edge. A `body[data-style="…"] .card` selector is more specific and
     wins. */

  /* Corner: the default. A panel in the bottom-right, at the size the palettes
     were originally authored for. */
  body[data-style="corner"] {
    --pad-y: 1.25rem;
    --pad-x: 1.5rem;
    --min-width: 16rem;
    --count-size: 3.5rem;
    --count-weight: 700;
    --count-spacing: normal;
    --corner-x: 2rem;
    --corner-y: 2rem;
  }

  /* Slim: the same panel, smaller, and in the top-left. Every palette is
     available in it, which is the point — this used to be a theme of its own
     (`minimal`), and could only ever be had in one colour. */
  body[data-style="slim"] {
    --pad-y: 0.75rem;
    --pad-x: 1rem;
    --min-width: 12rem;
    --count-size: 2.5rem;
    --count-weight: 600;
    --corner-x: 2rem;
    --corner-y: 2rem;
  }
  body[data-style="slim"] .card {
    /* Top-left, and only because a small block reads better there than tucked
       into a corner beside whatever else is on screen.

       In a rule of its own rather than in the variable block above, for the
       reason spelled out on `big` below: the base `.card` rule sets `right` and
       `bottom` directly, two selectors of equal specificity are settled by
       source order, and positioning declared earlier in the file loses to it.
       Declared here, this selector is more specific and wins.

       All four edges are set because two are already set by the base rule, and
       an `auto` that is never written does not cancel one that was. */
    left: var(--corner-x);
    top: var(--corner-y);
    right: auto;
    bottom: auto;
  }

  /* Strip: one line, edge to edge, along the bottom. For a channel with an
     alert or a chat box already in the corner — every other style wants the same
     real estate, and a streamer who cannot move it needs one that does not. It
     is a row rather than a column, so the bar grows into whatever width is left
     over instead of sitting under a number. */
  body[data-style="strip"] {
    --pad-y: 0.375rem;
    --pad-x: 1rem;
    --count-size: 1.375rem;
    --count-weight: 700;
    --corner-x: 0;
    --corner-y: 0;
  }
  body[data-style="strip"] .card {
    /* Row, full width, pinned to the bottom edge. `min-width` goes because this
       is as wide as the source, and `max-width` because the card *is* the strip
       — left in place it would cap the row at 28rem and defeat the whole style.
     */
    display: flex;
    flex-direction: row;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.75rem;
    left: 0;
    right: 0;
    top: auto;
    bottom: 0;
    min-width: 0;
    max-width: none;
  }
  body[data-style="strip"] .bar {
    /* The only thing that grows. Everything else keeps its natural width, so
       the bar absorbs whatever the words and the digits did not need. */
    flex: 1 1 8rem;
    min-width: 4rem;
    margin: 0;
  }
  body[data-style="strip"] .status,
  body[data-style="strip"] .count,
  body[data-style="strip"] .row,
  body[data-style="strip"] .event,
  body[data-style="strip"] .note {
    /* Without this a long event line squeezes the bar rather than the text
       around it, and the one thing that must stay readable is the one that
       shrinks. */
    flex: 0 0 auto;
    margin-top: 0;
  }
  body[data-style="strip"] .row {
    /* The two halves of this row are already separated by the gap; without this
       they spread to the far edges of a full-width strip. */
    gap: 0.75rem;
  }

  /* Big: one number, centred, nothing else.

     A different proposition rather than a different palette — the count alone,
     at a size readable from across a room, with no bar and no chrome. The number
     carries the progress on its own: "5,000 / 10,000" says the same thing the
     bar does, and a bar under a number that large is a second answer to a
     question already answered.

     It keeps a plate behind the digits, for the same reason every other style
     does: no card at all means white text over whatever is happening in the
     scene, which is unreadable far more often than it is not. */
  body[data-style="big"] {
    --pad-y: 1rem;
    --pad-x: 2rem;
    --count-size: 7rem;
    --count-weight: 800;
    --count-spacing: -0.03em;
    --corner-y: 2rem;
  }
  body[data-style="big"] .card {
    /* Shrink to the digits, then centre *that*. `width: fit-content` has to come
       first: with `left: 0; right: 0` alone the card box spans the whole source
       and the dark plate runs edge to edge, so the number is centred inside a
       full-width bar rather than a plate around the number.

       Centred by layout rather than arithmetic. `left: 50%` alone would put the
       card's *left edge* at the middle, and the usual repair —
       `transform: translateX(-50%)` — **removed the card from the screen
       entirely**: with the base rule's `min-width` in play the shift lands the
       whole thing off-canvas. A `transform` here is also the one property that
       would promote the card to its own compositing layer, which is the
       mechanism the `filter` and `text-shadow` bans exist to avoid. Auto margins
       distribute real slack, so nothing can fall out of step with the digit
       count. */
    width: fit-content;
    max-width: calc(100% - 4rem);
    left: 0;
    right: 0;
    top: auto;
    bottom: var(--corner-y);
    margin-inline: auto;
    /* Cleared because a minimum width beats the free space `margin-inline: auto`
       distributes, which is the whole mechanism being used to centre this. */
    min-width: 0;
    /* Keeps "/ 10,000" beside the count rather than wrapped under it. At 7rem
       the digits are wide enough that a wrap is easy to reach, and a target
       that has dropped to its own line reads as a second number. */
    white-space: nowrap;
  }
  body[data-style="big"] .status,
  body[data-style="big"] .bar,
  body[data-style="big"] .row {
    /* The parts that would make this the corner style again. */
    display: none;
  }
  body[data-style="big"] .count,
  body[data-style="big"] .event,
  body[data-style="big"] .note {
    text-align: center;
  }
  body[data-style="big"] .event {
    margin-top: 0.25rem;
  }
  body[data-style="big"] .note {
    margin-top: 0.5rem;
  }

</style>
</head>
<body>
  <!-- Starts visible, not hidden.
     The card used to start with `display: none` and be revealed by the first
     successful render. In OBS that meant a source could sit there showing
     nothing at all, with no way to tell "no yapathon running" from "this page is
     broken". Starting visible with a real message means the source always shows
     something, and what it shows says which case it is. -->
<div class="card" id="card">
    <div class="status" id="status">yapping</div>
    <div class="count"><span id="words">0</span> <span class="target" id="target"></span></div>
    <div class="bar"><div class="fill" id="fill"></div></div>
    <div class="row">
      <span id="remaining"></span>
      <span id="rate"></span>
    </div>
    <div class="event hidden" id="event"></div>
    <div class="note hidden" id="capped">goal capped — no more events can extend it</div>
  </div>

<script>
const card = document.getElementById('card');
const statusEl = document.getElementById('status');
const wordsEl = document.getElementById('words');
const targetEl = document.getElementById('target');
const fillEl = document.getElementById('fill');
const remainingEl = document.getElementById('remaining');
const rateEl = document.getElementById('rate');
const eventEl = document.getElementById('event');
const cappedEl = document.getElementById('capped');

let lastEventAt = 0;

function formatDuration(seconds) {
  if (seconds == null) return '';
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

function render(state) {
  card.className = 'card';

  if (!state) {
    // No yapathon. Still visible, so an empty source is distinguishable from a
    // broken one.
    statusEl.textContent = 'no yapathon';
    wordsEl.textContent = '—';
    targetEl.textContent = '';
    fillEl.style.transform = 'scaleX(0)';
    remainingEl.textContent = 'start one from the dock';
    rateEl.textContent = '';
    return;
  }

  card.className = 'card ' + state.status;

  statusEl.textContent = state.status_label;

  const isTime = state.mode.kind === 'time';
  if (isTime) {
    // A time run shows the clock, not a word total: what the streamer is
    // watching is how long is left, not how much they have said.
    const elapsed = state.elapsed_secs;
    const target = state.target;
    wordsEl.textContent = formatDuration(Math.max(0, target - elapsed));
    targetEl.textContent = 'left';
    remainingEl.textContent = `${state.utterances} utterances`;
    rateEl.textContent = state.words_per_minute
      ? `${Math.round(state.words_per_minute)} wpm`
      : '';
  } else {
    wordsEl.textContent = state.words.toLocaleString();
    targetEl.textContent = `/ ${state.target.toLocaleString()}`;
    remainingEl.textContent = `${state.remaining.toLocaleString()} to go`;
    rateEl.textContent = state.words_per_minute
      ? `${Math.round(state.words_per_minute)} wpm`
      : '';
  }

  fillEl.style.transform = 'scaleX(' + state.fraction + ')';

  // The streamer and chat both need to know the run is over by design rather
  // than stalled, or they will keep extending a goal nothing is counting.
  cappedEl.classList.toggle('hidden', !state.capped);
}

function showEvent(event) {
  eventEl.textContent = `+${event.words.toLocaleString()} ${event.kind}` +
    (event.user ? ` — ${event.user}` : '');
  eventEl.classList.remove('hidden');
  lastEventAt = Date.now();
}

function connect() {
  const socket = new WebSocket(`ws://${location.host}/api/ws`);

  socket.onmessage = (message) => {
    render(JSON.parse(message.data));
  };

  // OBS's CEF reconnects less eagerly than a browser, so do it here: a dropped
  // socket would otherwise freeze the overlay for the rest of the stream.
  socket.onclose = () => setTimeout(connect, 2000);
}

connect();

// Fetch once on load too, so the overlay is correct before the socket opens.
//
// This is what makes the overlay work at all in OBS: OBS's CEF blocks
// WebSockets to a private-network address on some configurations, and a source
// whose only feed is the socket then shows its initial state forever. Polling is
// the floor and the socket is the fast path, so either one alone is enough.
fetch('/api/state')
  .then((r) => r.json())
  .then((body) => render(body.snapshot))
  .catch(() => {
    statusEl.textContent = 'cannot reach yap-a-thon';
    remainingEl.textContent = 'is the app still running?';
  });

// Was the look pinned in this page's own URL?
//
// If it was, that is a deliberate choice — the streamer copied a URL with a
// theme in it — and nothing the app says afterwards should override it. If it
// was not, the overlay follows the app's setting, so changing it does not mean
// deleting and re-adding a Browser Source mid-stream.
const pinned = new URLSearchParams(location.search);

// What a streamer set in OBS's "Custom CSS" box, if anything.
//
// OBS injects that box as a `<style>` element *appended to `<head>`* — verified
// against the `obs-browser` plugin binary, which does exactly this in
// `document.querySelector('head').appendChild(obsCSS)`. Two consequences this
// code depends on:
//
//   - Being later in the document, it wins ties against our own stylesheet, so
//     someone who wants a colour we did not think of needs `:root { --accent: … }`
//     and nothing more.
//   - It never removes an attribute, so the override has to be read out of the
//     cascade and applied by hand. Hence reading the properties rather than
//     expecting the attribute to change on its own.
//
// Precedence: Custom CSS beats the app's setting, which beats nothing. The
// reason is that the CSS box is per-Browser-Source — someone can put a catppuccin
// overlay on one scene and the app's default on another, which no single app
// setting can express.
function cssOverride(axis) {
  const declared = getComputedStyle(document.documentElement)
    .getPropertyValue('--yap-' + axis);
  return (declared || '').trim();
}

// Adopt whatever the CSS box asked for. Absent variables leave the attributes
// alone, which is what makes "set one axis, leave the other" work.
function applyCssOverrides() {
  const theme = cssOverride('theme');
  if (theme) document.body.dataset.theme = theme;

  const style = cssOverride('style');
  if (style) document.body.dataset.style = style;
}

// Run once now and again on the poll: OBS injects the CSS box on load, and the
// exact ordering against this script is not something to rely on.
applyCssOverrides();

// Poll as well as subscribing. Cheap: a few bytes, and the interval only matters
// when the socket is unavailable.
setInterval(() => {
  fetch('/api/state')
    .then((r) => r.json())
    .then((body) => {
      render(body.snapshot);
      applyCssOverrides();

      if (!pinned.has('theme') && body.overlay_theme &&
          document.body.dataset.theme !== body.overlay_theme) {
        // Only the attribute changes: every palette is already in this page's
        // stylesheet, so there is nothing to reload and nothing to flash.
        document.body.dataset.theme = body.overlay_theme;
      }

      if (!pinned.has('style') && body.overlay_style &&
          document.body.dataset.style !== body.overlay_style) {
        document.body.dataset.style = body.overlay_style;
      }
    })
    .catch(() => {});
}, 2000);

// A bonus line has to fade, or the last sub of the stream sits on screen for
// the rest of it.
setInterval(() => {
  if (lastEventAt && Date.now() - lastEventAt > 6000) {
    eventEl.classList.add('hidden');
    lastEventAt = 0;
  }
}, 1000);
</script>
</body>
</html>
"##;

/// The control dock, opened as an OBS Custom Dock or a normal browser tab.
///
/// Richer than the overlay because it is read by the streamer, not the audience:
/// controls, the transcript, the event feed and settings.
const DOCK_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>yap-a-thon</title>
<style>
  :root {
    --bg: #16161a;
    --panel: #1e1e24;
    --fg: #f4f4f5;
    --dim: #a1a1aa;
    --accent: #4ade80;
    --warn: #fbbf24;
    --danger: #f87171;
    --border: rgba(255, 255, 255, 0.09);
  }
  * { box-sizing: border-box; }
  body {
    margin: 0;
    background: var(--bg);
    color: var(--fg);
    font: 14px/1.5 ui-sans-serif, system-ui, -apple-ui, sans-serif;
  }
  header {
    padding: 1rem 1.25rem;
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: baseline;
    gap: 0.75rem;
  }
  h1 { font-size: 1rem; margin: 0; font-weight: 600; }
  .status { color: var(--dim); font-size: 0.8rem; }
  main { padding: 1.25rem; display: grid; gap: 1.25rem; }
  .panel { background: var(--panel); border: 1px solid var(--border); border-radius: 0.5rem; padding: 1rem; }
  .count { font-size: 2.75rem; font-weight: 700; font-variant-numeric: tabular-nums; line-height: 1.1; }
  .target { color: var(--dim); font-size: 0.4em; font-weight: 400; }
  .bar { height: 0.5rem; background: rgba(255,255,255,0.08); border-radius: 999px; overflow: hidden; margin: 0.75rem 0; }
  .fill { height: 100%; background: var(--accent); transition: width 250ms ease-out; width: 0; }
  .stats { display: flex; flex-wrap: wrap; gap: 1.25rem; font-size: 0.85rem; color: var(--dim); }
  .stats b { display: block; color: var(--fg); font-size: 1.1rem; font-variant-numeric: tabular-nums; }
  .controls { display: flex; flex-wrap: wrap; gap: 0.5rem; }
  button {
    font: inherit;
    padding: 0.45rem 0.9rem;
    border-radius: 0.375rem;
    border: 1px solid var(--border);
    background: #26262e;
    color: var(--fg);
    cursor: pointer;
  }
  button:hover { background: #30303a; }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  button.primary { background: var(--accent); color: #06240f; border-color: transparent; font-weight: 600; }
  button.danger { color: var(--danger); }
  input, select {
    font: inherit;
    padding: 0.4rem 0.6rem;
    border-radius: 0.375rem;
    border: 1px solid var(--border);
    background: #101014;
    color: var(--fg);
  }
  form { display: flex; flex-wrap: wrap; gap: 0.5rem; align-items: center; }
  .feed { max-height: 16rem; overflow-y: auto; font-size: 0.85rem; }
  .feed li { padding: 0.3rem 0; border-bottom: 1px solid var(--border); list-style: none; }
  .feed ul { margin: 0; padding: 0; }
  .warn { color: var(--warn); }
  .error { color: var(--danger); }
  .muted { color: var(--dim); }
  .transcript { max-height: 20rem; overflow-y: auto; font-size: 0.85rem; }
  .transcript p { margin: 0 0 0.4rem; }
  .voided { text-decoration: line-through; color: var(--dim); }
  label { display: inline-flex; align-items: center; gap: 0.4rem; font-size: 0.85rem; color: var(--dim); }
</style>
</head>
<body>
<header>
  <h1>yap-a-thon</h1>
  <span class="status" id="status">no yapathon</span>
</header>

<main>
  <section class="panel">
    <div class="count"><span id="words">0</span> <span class="target" id="target">words</span></div>
    <div class="bar"><div class="fill" id="fill"></div></div>
    <div class="stats">
      <div><b id="remaining">0</b> to go</div>
      <div><b id="rate">—</b> words/min</div>
      <div><b id="elapsed">0:00</b> elapsed</div>
      <div><b id="score">0</b> from chat</div>
      <div><b id="fillers">0</b> fillers</div>
    </div>
    <div id="caps" class="warn" hidden>goal capped — no more events can extend it</div>
    <div id="behind" class="warn" hidden>catching up — the counter is behind your voice</div>
    <div id="obs" class="muted" style="margin-top:0.75rem;font-size:0.75rem">
      Add a <strong>Browser Source</strong> in OBS with this URL, sized to your scene
      (e.g. 480×270). It must be at least the width of the number.
    </div>
  </section>

  <section class="panel">
    <form id="start">
      <label>Mode
        <select id="mode">
          <option value="extend">chat extends goal</option>
          <option value="fixed">fixed goal</option>
          <option value="time">chat extends clock</option>
        </select>
      </label>
      <label>Goal <input id="goal" type="number" value="5000" min="1" step="100"></label>
      <label>Limit <input id="limit" type="number" value="10000" min="0" step="100"></label>
      <button class="primary" type="submit">Start</button>
    </form>
    <div class="controls" style="margin-top:0.75rem">
      <button data-action="pause">Pause</button>
      <button data-action="resume">Resume</button>
      <button data-action="undo">Undo last</button>
      <button data-action="end" class="danger">End</button>
    </div>
    <form id="adjust" style="margin-top:0.75rem">
      <label>Adjust <input id="adjustWords" type="number" value="0" step="1"></label>
      <label>Note <input id="adjustNote" type="text" placeholder="why"></label>
      <button type="submit">Apply</button>
    </form>
  </section>

  <section class="panel">
    <h2 class="muted" style="font-size:0.8rem;margin:0 0 0.5rem">Chat events</h2>
    <div class="feed"><ul id="feed"></ul></div>
  </section>

  <section class="panel">
    <h2 class="muted" style="font-size:0.8rem;margin:0 0 0.5rem">Transcript</h2>
    <div class="transcript" id="transcript"></div>
  </section>

  <section class="panel">
    <h2 class="muted" style="font-size:0.8rem;margin:0 0 0.5rem">Warnings</h2>
    <div id="warnings" class="feed"></div>
  </section>
</main>

<script>
const el = (id) => document.getElementById(id);

function formatDuration(seconds) {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  return h > 0 ? `${h}:${String(m).padStart(2,'0')}:${String(s).padStart(2,'0')}`
               : `${m}:${String(s).padStart(2,'0')}`;
}

function render(state) {
  if (!state) {
    el('status').textContent = 'no yapathon';
    return;
  }

  el('status').textContent = `${state.status_label} · ${state.mode_label}`;
  const isTime = state.mode.kind === 'time';

  el('words').textContent = isTime
    ? formatDuration(Math.max(0, state.target - state.elapsed_secs))
    : state.words.toLocaleString();
  el('target').textContent = isTime
    ? 'left'
    : `/ ${state.target.toLocaleString()} words`;
  el('fill').style.transform = 'scaleX(' + state.fraction + ')';
  el('remaining').textContent = isTime
    ? `${state.utterances} utterances`
    : state.remaining.toLocaleString();
  el('rate').textContent = state.words_per_minute
    ? Math.round(state.words_per_minute)
    : '—';
  el('elapsed').textContent = formatDuration(state.elapsed_secs);
  el('score').textContent = (state.score + state.bonus_total).toLocaleString();
  el('fillers').textContent = state.fillers.toLocaleString();
  el('caps').hidden = !state.capped;
}

async function loadEvents() {
  const response = await fetch('/api/events/log');
  const events = await response.json();
  const feed = el('feed');
  feed.innerHTML = '';
  for (const event of events.slice(-40).reverse()) {
    const item = document.createElement('li');
    const who = event.user ? ` — ${event.user}` : '';
    const words = event.words != null ? `+${event.words} words` : 'no effect';
    item.textContent = `${event.kind}${who}: ${words}`;
    feed.appendChild(item);
  }
}

async function loadTranscript() {
  const response = await fetch('/api/transcript');
  const lines = await response.json();
  const box = el('transcript');
  box.innerHTML = '';
  for (const line of lines.slice(-80).reverse()) {
    const p = document.createElement('p');
    p.textContent = `${line.words} words @ ${formatDuration(line.started_ms / 1000)} — ${line.text}`;
    if (line.voided) p.className = 'voided';
    box.appendChild(p);
  }
}

async function loadWarnings() {
  const response = await fetch('/api/warnings');
  const warnings = await response.json();
  const box = el('warnings');
  box.innerHTML = '';
  if (warnings.length === 0) {
    box.innerHTML = '<span class="muted">none</span>';
    return;
  }
  for (const warning of warnings.slice(-20).reverse()) {
    const p = document.createElement('p');
    p.textContent = warning;
    box.appendChild(p);
  }
}

async function post(path, body) {
  const options = body
    ? { method: 'POST', headers: {'content-type': 'application/json'}, body: JSON.stringify(body) }
    : { method: 'POST' };
  const response = await fetch(path, options);
  if (!response.ok) {
    const problem = await response.json().catch(() => ({error: response.statusText}));
    throw new Error(problem.error || 'request failed');
  }
  return response.json();
}

el('start').addEventListener('submit', async (e) => {
  e.preventDefault();
  const mode = el('mode').value;
  const goal = Number(el('goal').value);
  const limitRaw = el('limit').value;
  const hardLimit = limitRaw ? Number(limitRaw) : null;

  const body = {
    hard_limit: hardLimit,
    mode: mode === 'fixed' ? { kind: 'fixed', words: goal }
         : mode === 'time' ? { kind: 'time', secs: goal }
         : { kind: 'extend', base_words: goal },
  };

  try {
    render((await post('/api/start', body)).snapshot);
    loadEvents();
  } catch (problem) {
    el('status').textContent = problem.message;
    el('status').className = 'error';
  }
});

el('adjust').addEventListener('submit', async (e) => {
  e.preventDefault();
  const words = Number(el('adjustWords').value);
  if (words === 0) return;
  try {
    render((await post('/api/adjust', { words, note: el('adjustNote').value })).snapshot);
  } catch (problem) {
    el('status').textContent = problem.message;
  }
});

for (const button of document.querySelectorAll('[data-action]')) {
  button.addEventListener('click', async () => {
    const action = button.dataset.action;
    try {
      if (action === 'undo') {
        const result = await post('/api/undo');
        if (!result.ok) {
          el('status').textContent = 'nothing to undo';
          return;
        }
        render(result.snapshot);
        loadTranscript();
      } else {
        render((await post(`/api/${action}`)).snapshot);
      }
    } catch (problem) {
      el('status').textContent = problem.message;
    }
  });
}

function connect() {
  const socket = new WebSocket(`ws://${location.host}/api/ws`);
  socket.onmessage = (message) => render(JSON.parse(message.data));
  socket.onclose = () => setTimeout(connect, 2000);
}

async function boot() {
  const body = await fetch('/api/state').then((r) => r.json());
  render(body.snapshot);
  loadEvents();
  loadTranscript();
  loadWarnings();
  connect();
}

boot();
</script>
</body>
</html>
"##;

/// Serve the overlay page.
///
/// `no-store` on both pages: a browser source that caches the HTML will keep
/// showing an old build after the app updates, and there is no cache-busting URL
/// in OBS to work around it.
///
/// `?debug=1` swaps in a diagnostic page that paints the whole canvas magenta
/// and prints the URL, the viewport size and whether the API answered. It is the
/// fastest way to tell "OBS is not loading my page" from "OBS is loading my
/// page and something in it does not render" — two problems that look identical
/// from the outside and need opposite fixes.
pub async fn serve_overlay(
    State(state): State<Arc<AppState>>,
    raw: axum::extract::RawQuery,
) -> Response {
    // Parsed by hand rather than through a `Query<Params>` extractor.
    //
    // A typed extractor rejects the whole request with 400 on anything it cannot
    // deserialise — including `?debug` with no `=`, and `?debug=1`, which is not
    // a serde `bool`. Both are things a person types into a URL bar. The overlay
    // must never fail to serve over a query string, because the overlay is the
    // thing that has to work.
    // `RawQuery` is `Option<String>` in this axum, and an absent query is an
    // empty one for our purposes.
    let query = raw.0.as_deref().unwrap_or("");

    // `?theme=` and `?style=` beat the setting, so a streamer can compare looks
    // by editing the URL in a browser without touching anything and then copy
    // the one they want into OBS. Either can be pinned alone — `?theme=neon`
    // keeps whichever shape the app is set to.
    let (theme, style) = state.overlay_look(
        value(query, "theme").as_deref(),
        value(query, "style").as_deref(),
    );

    let body = if flag(query, "debug") {
        debug_html()
    } else if flag(query, "opaque") {
        opaque_overlay_html(Some(&theme))
    } else {
        overlay_html_with(&theme, Some(&style))
    };

    Html(body)
        .into_response()
        .with_header(header::CACHE_CONTROL, "no-store, max-age=0")
}

/// The value of a key in a raw query string.
fn value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key && !value.is_empty()).then(|| value.to_string())
    })
}

/// Whether a flag is set in a raw query string.
///
/// Presence alone is enough: `?debug`, `?debug=1`, `?debug=true` and `?debug=yes`
/// all mean yes. An explicit `0`, `false`, `no` or `off` mean no, so a link can
/// turn a flag off rather than only on.
fn flag(query: &str, key: &str) -> bool {
    query.split('&').any(|pair| {
        let (name, value) = match pair.split_once('=') {
            Some((name, value)) => (name, value),
            None => (pair, ""),
        };

        if name != key {
            return false;
        }

        !matches!(value.to_lowercase().as_str(), "0" | "false" | "no" | "off")
    })
}

/// Serve the control dock.
pub async fn serve_dock(State(_state): State<Arc<AppState>>) -> Response {
    Html(DOCK_HTML)
        .into_response()
        .with_header(header::CACHE_CONTROL, "no-store, max-age=0")
}

/// One overlay look.
///
/// The CSS lives in [`OVERLAY_HTML`] as `body[data-theme="id"]` rules rather
/// than being generated from this table. Two sources of truth would drift, so
/// this table is the one the settings panel and the tests read, and a test
/// checks that every theme here has a rule in the stylesheet and every rule in
/// the stylesheet is in here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayPalette {
    /// The value stored in settings and passed as `?theme=`.
    pub id: &'static str,
    /// What the settings panel calls it.
    pub label: &'static str,
    /// One line on what it is for.
    pub note: &'static str,
}

impl std::fmt::Display for OverlayPalette {
    /// The id, so a test failure can say "{palette}/{style}" and be read.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id)
    }
}

/// Every palette, in the order the settings panel lists them.
///
/// **Colours and nothing else.** A palette never changes the shape — that is
/// [`OverlayStyle`]'s job — which is what lets any of them pair with any style.
/// `card` is first and is the default: an install that has never heard of
/// either axis keeps exactly the look it had.
pub const OVERLAY_PALETTES: &[OverlayPalette] = &[
    OverlayPalette {
        id: "card",
        label: "Card",
        note: "the default: dark, with a green accent",
    },
    OverlayPalette {
        id: "minimal",
        label: "Minimal",
        note: "near-black and cool blue",
    },
    OverlayPalette {
        id: "cute",
        label: "Cute",
        note: "pastel rose on a pale pink card",
    },
    OverlayPalette {
        id: "neon",
        label: "Neon",
        note: "one electric cyan accent on black",
    },
    OverlayPalette {
        id: "mono",
        label: "Mono",
        note: "greyscale, for a channel that is already loud",
    },
    OverlayPalette {
        id: "catppuccin-mocha",
        label: "Catppuccin Mocha",
        note: "warm pastel dark, with a glow",
    },
    OverlayPalette {
        id: "catppuccin-macchiato",
        label: "Catppuccin Macchiato",
        note: "a cooler, lower-contrast catppuccin",
    },
];

/// The palette used when nothing is configured.
pub const DEFAULT_OVERLAY_PALETTE: &str = "card";

/// Look up a palette by id, falling back to the default.
///
/// Falling back rather than failing, because the id comes out of a settings file
/// a person can edit. An unknown name must not take the overlay down: it is the
/// only thing on screen during a stream.
pub fn overlay_palette(id: &str) -> &'static OverlayPalette {
    OVERLAY_PALETTES
        .iter()
        .find(|palette| palette.id == id.trim().to_lowercase())
        .unwrap_or_else(|| {
            log::warn!("overlay: unknown palette {id:?}, using {DEFAULT_OVERLAY_PALETTE:?}");
            palette_by_id(DEFAULT_OVERLAY_PALETTE)
        })
}

fn palette_by_id(id: &str) -> &'static OverlayPalette {
    OVERLAY_PALETTES
        .iter()
        .find(|palette| palette.id == id)
        .expect("the default palette is in the table")
}

/// One overlay shape.
///
/// The second axis. A style decides where the card sits, how much of it there
/// is, and which parts are drawn — never what colour anything is. Keeping the
/// two apart is what makes `slim` a single block of CSS instead of one per
/// palette, and it is why a streamer can have a slim catppuccin overlay without
/// anyone writing it for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayStyle {
    /// The value stored in settings and passed as `?style=`.
    pub id: &'static str,
    /// What the settings panel calls it.
    pub label: &'static str,
    /// One line on what it is for.
    pub note: &'static str,
}

impl std::fmt::Display for OverlayStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id)
    }
}

/// Every style, in the order the settings panel lists them.
pub const OVERLAY_STYLES: &[OverlayStyle] = &[
    OverlayStyle {
        id: "corner",
        label: "Corner",
        note: "the default: a rounded panel in the bottom-right",
    },
    OverlayStyle {
        id: "slim",
        label: "Slim",
        note: "the same panel, smaller and in the top-left",
    },
    OverlayStyle {
        id: "strip",
        label: "Strip",
        note: "one line across the bottom, for a busy corner",
    },
    OverlayStyle {
        id: "big",
        label: "Big",
        note: "just the number, centred and large",
    },
];

/// The style used when nothing is configured.
pub const DEFAULT_OVERLAY_STYLE: &str = "corner";

/// Look up a style by id, falling back to the default.
///
/// Falling back for the same reason [`overlay_palette`] does.
pub fn overlay_style(id: &str) -> &'static OverlayStyle {
    OVERLAY_STYLES
        .iter()
        .find(|style| style.id == id.trim().to_lowercase())
        .unwrap_or_else(|| {
            log::warn!("overlay: unknown style {id:?}, using {DEFAULT_OVERLAY_STYLE:?}");
            style_by_id(DEFAULT_OVERLAY_STYLE)
        })
}

fn style_by_id(id: &str) -> &'static OverlayStyle {
    OVERLAY_STYLES
        .iter()
        .find(|style| style.id == id)
        .expect("the default style is in the table")
}

/// Every `(palette, style)` pair that used to be a single theme.
///
/// # Why this exists
///
/// Before the two axes there was one list of combined names, and a
/// `settings.toml` written then still says `overlay_theme = "minimal"`. Reading
/// that as a palette id alone would give someone who had never chosen a palette
/// a **strip overlay** — because `strip` and `big` are styles, and would be
/// found as such — which is the worst possible outcome: the overlay silently
/// changes shape for someone who upgraded mid-stream.
///
/// So the old names are mapped to the pair that reproduces what they used to
/// see. Unknown names are *not* mapped here; they fall back to the defaults,
/// which is already the behaviour and already logged.
pub fn legacy_look(id: &str) -> Option<(&'static str, &'static str)> {
    Some(match id.trim().to_lowercase().as_str() {
        "card" => ("card", "corner"),
        "minimal" => ("minimal", "slim"),
        "cute" => ("cute", "corner"),
        "neon" => ("neon", "corner"),
        "mono" => ("mono", "corner"),
        "strip" => ("card", "strip"),
        "big" => ("card", "big"),
        _ => return None,
    })
}

/// Every old combined name, so a settings file can be migrated on read.
pub const LEGACY_LOOKS: &[&str] = &["card", "minimal", "cute", "neon", "mono", "strip", "big"];

/// Look up a palette by id, falling back to the default.
///
/// Kept as the old name because `settings.toml` and every existing caller speak
/// in combined looks, and a caller asking for `"minimal"` means the pair.
pub fn overlay_theme(id: &str) -> &'static OverlayPalette {
    overlay_palette(id)
}

/// The old accessor kept compiling for the settings panel and tests.
///
/// # Why the split is worth this
///
/// `OVERLAY_THEMES` cannot mean the same thing twice over. The settings panel
/// and the stylesheet both iterate it, and they now need *different* lists — the
/// panel offers two dropdowns and the stylesheet keys on two attributes. Rather
/// than thread two iterators through both, each keeps the one it wants and the
/// agreement test checks each against its own.
pub const OVERLAY_THEMES: &[OverlayPalette] = OVERLAY_PALETTES;

/// The overlay markup, exposed for tests.
pub fn overlay_html() -> &'static str {
    OVERLAY_HTML
}

/// The overlay markup with a look applied.
///
/// The body carries `data-theme` and `data-style`, which is all the stylesheet
/// keys off — so every combination is present in the served page and switching is
/// two attributes rather than a second copy of the CSS.
pub fn overlay_html_themed(id: &str) -> String {
    overlay_html_with(id, None)
}

/// The overlay markup with both axes, for tests.
///
/// `style` is separate because that is how it is chosen at runtime: a settings
/// file may name a style, a URL may pin one, or a streamer may set
/// `--yap-style` in OBS's Custom CSS box, and each of those arrives on its own.
pub fn overlay_html_with(theme: &str, style: Option<&str>) -> String {
    let (palette, style) = match legacy_look(theme) {
        Some(pair) if style.is_none() => (pair.0, Some(pair.1)),
        _ => (
            overlay_palette(theme).id,
            style.map(|id| overlay_style(id).id),
        ),
    };

    let body = format!(
        r#"<body data-theme="{palette}" data-style="{}">"#,
        style.unwrap_or(DEFAULT_OVERLAY_STYLE)
    );

    OVERLAY_HTML.replace("<body>", &body)
}

/// The dock markup, exposed for tests.
pub fn dock_html() -> &'static str {
    DOCK_HTML
}

/// The overlay with a fully opaque card.
///
/// For a machine where the alpha in `rgba(16,16,20,0.92)` composites to nothing.
/// The body's transparency is untouched — that is what lets the stream show
/// through — only the card becomes solid.
fn opaque_overlay_html(theme: Option<&str>) -> String {
    let html = overlay_html_with(theme.unwrap_or(DEFAULT_OVERLAY_PALETTE), None);

    // The variable holding whatever the chosen palette uses, so this works for a
    // pastel one as well as the dark default.
    html.replace("var(--card);", "var(--card-solid);")
}

/// Remove CSS and JS comments from a page.
///
/// Test-only, and only because the banned-property test needs to see the
/// declarations rather than the prose explaining them.
#[cfg(test)]
fn strip_css_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Both comment forms, since the pages use `/* */` in CSS and `//` in JS.
        if bytes[i..].starts_with(b"/*") {
            match html[i..].find("*/") {
                Some(end) => i += end + 2,
                None => break,
            }
            continue;
        }

        if bytes[i..].starts_with(b"//") {
            match html[i..].find('\n') {
                Some(end) => i += end,
                None => break,
            }
            continue;
        }

        // Advance one char, not one byte, so multi-byte text survives.
        let ch = html[i..].chars().next().expect("a char");
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

/// A diagnostic page: solid magenta, enormous text, no CSS that could be the
/// reason it fails.
///
/// Deliberately crude. A debug page that depends on anything the real overlay
/// depends on cannot distinguish the two, so this paints with nothing but
/// `background` and writes text with default styles.
fn debug_html() -> String {
    r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>yap-a-thon overlay — debug</title>
<style>
  /* No custom properties, no filters, no transforms, no flexbox. If this paints,
     OBS is loading the page and the problem is in the overlay's own styling. */
  html, body {
    margin: 0;
    padding: 0;
    background: #ff00ff;
    height: 100%;
  }
  body {
    font-family: monospace;
    color: #000000;
  }
  pre {
    margin: 0;
    padding: 12px;
    font-size: 20px;
    line-height: 1.4;
    white-space: pre-wrap;
    word-break: break-all;
  }
</style>
</head>
<body>
<pre id="out">yap-a-thon debug
checking the API...</pre>
<script>
var out = document.getElementById('out');

function report(lines) {
  out.textContent = 'yap-a-thon debug\n\n' + lines.join('\n');
}

report([
  'url:      ' + location.href,
  'viewport: ' + innerWidth + 'x' + innerHeight,
  'user agent contains CEF: ' + (/CEF|Chrome/.test(navigator.userAgent)),
  '',
  'api: checking...'
]);

// Whether the server is reachable at all, from inside OBS's own network stack.
// A fetch that fails here is the "app is not running" case; one that succeeds
// means OBS can talk to the app and the fault is in rendering.
fetch('/api/state')
  .then(function (r) { return r.ok ? r.json() : Promise.reject(new Error('status ' + r.status)); })
  .then(function (body) {
    var s = body.snapshot;
    report([
      'url:      ' + location.href,
      'viewport: ' + innerWidth + 'x' + innerHeight,
      '',
      'api: OK',
      'yapathon: ' + (s ? (s.status + ', ' + s.words + '/' + s.target + ' words') : 'none running')
    ]);
  })
  .catch(function (e) {
    report([
      'url:      ' + location.href,
      'viewport: ' + innerWidth + 'x' + innerHeight,
      '',
      'api: FAILED — ' + e.message,
      '',
      'The page loaded (you can read this) but the server did not answer.',
      'Start yap-a-thon and check the port in its settings.'
    ]);
  });
</script>
</body>
</html>
"##
    .to_string()
}

trait IntoHeaderValue {
    fn with_header(self, name: header::HeaderName, value: &'static str) -> Response;
}

impl IntoHeaderValue for Response {
    fn with_header(mut self, name: header::HeaderName, value: &'static str) -> Response {
        self.headers_mut()
            .insert(name, header::HeaderValue::from_static(value));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_are_read_leniently() {
        // People type these into a URL bar, so every spelling a person is likely
        // to use has to work. A 400 on `?debug` would look exactly like the bug
        // this page exists to diagnose.
        for yes in [
            "debug",
            "debug=",
            "debug=1",
            "debug=true",
            "debug=TRUE",
            "debug=yes",
        ] {
            assert!(flag(yes, "debug"), "{yes:?} should set the flag");
        }

        for no in ["debug=0", "debug=false", "debug=no", "debug=off"] {
            assert!(!flag(no, "debug"), "{no:?} should clear the flag");
        }

        assert!(!flag("", "debug"), "an empty query sets nothing");
        assert!(!flag("opaque=1", "debug"), "one flag must not set another");
        assert!(flag("opaque=1&debug=1", "debug"), "flags combine");
    }

    #[test]
    fn the_overlay_uses_nothing_obs_cannot_render() {
        // The bug that made this overlay invisible in OBS while looking perfect
        // in a browser: `backdrop-filter` promoted the card into a compositing
        // layer that CEF could return empty. The same applies to `filter`,
        // `text-shadow` and `mix-blend-mode`.
        //
        // Each of these has taken the overlay out of a scene at least once, and
        // none of them buys anything here — the card's job is to be legible over
        // a video, not to look like glass.
        // Comments are stripped first. This file explains *why* each property is
        // banned and names them in doing so, so a naive substring search finds
        // the explanation and fails on it — which is exactly the kind of test
        // that gets deleted the first time it is inconvenient.
        let html = strip_css_comments(overlay_html());

        for banned in [
            "backdrop-filter",
            "mix-blend-mode",
            "isolation:",
            "filter:",
            "text-shadow",
            "clip-path",
        ] {
            assert!(
                !html.contains(banned),
                "the overlay must not use {banned:?}: OBS's CEF does not render it reliably"
            );
        }
    }

    #[test]
    fn the_progress_bar_geometry_belongs_to_the_stylesheet() {
        // The fill is a full-width element scaled from the left, so filling it is
        // a compositor transform rather than a layout.
        //
        // That broke the moment the markup kept an inline `style="width:0%"`:
        // an inline style beats the stylesheet, so the element stayed zero wide
        // and the transform had nothing to scale. The bar rendered as a flat
        // grey line in every theme, which reads as "the feature is off" rather
        // than "a percentage is miscomputed" — and it did so silently, in all
        // five themes at once.
        let html = overlay_html();

        assert!(
            !html.contains(r#"id="fill" style="#),
            "the fill must have no inline style: it would override the stylesheet",
        );
        assert!(
            html.contains("transform: scaleX(0)"),
            "the fill must start collapsed by transform",
        );
        assert!(
            html.contains("transition: transform"),
            "the fill must animate a transform, not a width",
        );

        // And the script must write the same property the stylesheet animates.
        // Setting `width` while transitioning `transform` is the same bug with
        // the inline style removed.
        assert!(
            html.contains("fillEl.style.transform"),
            "the fill must be driven by transform",
        );
        assert!(
            !html.contains("fillEl.style.width"),
            "the fill must not be driven by width",
        );
    }

    #[test]
    fn the_overlay_restyles_itself_without_a_reload() {
        // OBS creates a Browser Source once and then leaves it alone. If changing
        // the theme meant re-adding the source, "pick a different look" would mean
        // "break the stream", which is a bad trade for a colour.
        //
        // Every theme is already in the served page, so restyling is one attribute
        // and no reload. The guard is the important half: a URL with `?theme=` is a
        // deliberate choice and nothing the app says afterwards may override it.
        let html = overlay_html();

        assert!(
            html.contains("pinned.has('theme')") && html.contains("pinned.has('style')"),
            "the overlay must know whether either axis was pinned in the URL",
        );
        assert!(
            html.contains("document.body.dataset.theme = body.overlay_theme"),
            "the overlay must adopt the served palette",
        );
        assert!(
            html.contains("document.body.dataset.style = body.overlay_style"),
            "the overlay must adopt the served shape — the two axes are separate \
             settings and either can change on its own",
        );
        assert!(
            !html.contains("location.reload"),
            "nothing may reload the page to restyle it",
        );

        // And every theme is present in the page regardless of which one is served,
        // which is what makes a live swap possible at all.
        for theme in OVERLAY_THEMES {
            assert!(
                html.contains(&format!(r#"body[data-theme="{}"]"#, theme.id)),
                "{} is not in the served page, so it cannot be swapped to live",
                theme.id,
            );
        }
    }

    #[test]
    fn the_served_state_carries_the_theme() {
        // Without this in the response the overlay has nothing to swap to, and the
        // poll would have to become a reload.
        let state = AppState::shared();
        state.set_overlay_theme("neon");
        assert_eq!(state.overlay_theme(None), "neon");

        // A pinned URL still wins, because that is the streamer having chosen it.
        assert_eq!(state.overlay_theme(Some("cute")), "cute");
        assert_eq!(
            state.overlay_theme(None),
            "neon",
            "the setting is unchanged"
        );
    }

    #[test]
    fn every_theme_has_a_rule_and_every_rule_has_a_theme() {
        // The names live in two places: a Rust table the settings panel reads,
        // and CSS rules in the served page. Two sources of truth drift, and a
        // theme in one and not the other is either invisible in the panel or
        // silently impossible to select.
        let html = overlay_html();

        for theme in OVERLAY_THEMES {
            assert!(
                html.contains(&format!(r#"body[data-theme="{}"]"#, theme.id)),
                "theme {} has no rule in the stylesheet",
                theme.id,
            );
        }

        // And nothing in the stylesheet that the panel cannot offer.
        for line in html
            .lines()
            .filter(|line| line.contains("body[data-theme="))
        {
            let quoted = line
                .split('"')
                .nth(1)
                .unwrap_or_else(|| panic!("unparseable theme rule: {line}"));
            assert!(
                OVERLAY_THEMES.iter().any(|theme| theme.id == quoted),
                "{quoted:?} has a stylesheet rule but is not in OVERLAY_THEMES",
            );
        }
    }

    #[test]
    fn every_palette_and_style_can_be_asked_for_and_both_defaults_are_real() {
        // Both axes have to be independently applicable, or the split is a
        // rename rather than a feature: `?theme=neon` must work whatever shape is
        // in force, and `?style=strip` whatever palette.
        for palette in OVERLAY_PALETTES {
            let html = overlay_html_with(palette.id, Some("corner"));
            assert!(
                html.contains(&format!(
                    r#"<body data-theme="{}" data-style="corner">"#,
                    palette.id
                )),
                "{} was not applied to the body",
                palette.id,
            );
        }

        for style in OVERLAY_STYLES {
            let html = overlay_html_with("card", Some(style.id));
            assert!(
                html.contains(&format!(r#"data-style="{}""#, style.id)),
                "{} was not applied to the body",
                style.id,
            );
        }

        assert!(
            OVERLAY_PALETTES
                .iter()
                .any(|palette| palette.id == DEFAULT_OVERLAY_PALETTE)
        );
        assert!(
            OVERLAY_STYLES
                .iter()
                .any(|style| style.id == DEFAULT_OVERLAY_STYLE)
        );
    }

    #[test]
    fn an_unknown_palette_or_style_falls_back_rather_than_failing() {
        // The ids can come out of a hand-edited settings file or a mistyped
        // query. An overlay that refuses to render is the one failure that
        // cannot happen during a stream.
        assert_eq!(overlay_palette("neon").id, "neon");
        assert_eq!(overlay_palette("NEON").id, "neon");
        assert_eq!(overlay_palette("  neon  ").id, "neon");
        assert_eq!(overlay_palette("not-a-theme").id, DEFAULT_OVERLAY_PALETTE);
        assert_eq!(overlay_palette("").id, DEFAULT_OVERLAY_PALETTE);

        assert_eq!(overlay_style("strip").id, "strip");
        assert_eq!(overlay_style("STRIP").id, "strip");
        assert_eq!(overlay_style("not-a-style").id, DEFAULT_OVERLAY_STYLE);
    }

    #[test]
    fn a_palette_never_sets_a_shape_and_a_style_never_sets_a_colour() {
        // The whole design rests on the two axes being independent, and nothing
        // about CSS stops a palette from quietly setting `--count-size` and
        // making `slim` meaningless for that one palette.
        //
        // The failure is invisible: every palette still has a rule, every style
        // still applies, and the overlay looks *almost* right — one theme simply
        // ignores the style someone picked.
        for palette in OVERLAY_PALETTES {
            let rule = theme_rule(palette.id).unwrap_or_default();

            for shape in [
                "--pad-y",
                "--pad-x",
                "--min-width",
                "--count-size",
                "--count-weight",
                "--corner-x",
                "--corner-y",
                "left:",
                "top:",
                "display:",
            ] {
                assert!(
                    !rule.contains(shape),
                    "palette {} sets {shape:?}, which belongs to the style — it would \
                     silently override the shape the streamer picked",
                    palette.id,
                );
            }
        }

        for style in OVERLAY_STYLES {
            let rule = theme_rule(style.id).unwrap_or_default();

            for colour in ["--card", "--fg", "--dim", "--accent", "--track", "--font"] {
                assert!(
                    !rule.contains(colour),
                    "style {} sets {colour:?}, which belongs to the palette — a slim \
                     catppuccin would not be catppuccin",
                    style.id,
                );
            }
        }
    }

    #[test]
    fn every_palette_pairs_with_every_style_and_the_combination_is_reachable() {
        // The point of the split: seven palettes times four styles is twenty-four
        // looks, and none of them needed twenty-four blocks of CSS. Checked
        // through the real renderer, because a combination that renders the
        // default on one axis would pass every per-axis test above.
        for palette in OVERLAY_PALETTES {
            for style in OVERLAY_STYLES {
                let html = overlay_html_with(palette.id, Some(style.id));

                assert!(
                    html.contains(&format!(r#"data-theme="{}""#, palette.id)),
                    "{palette}/{style} lost its palette",
                );
                assert!(
                    html.contains(&format!(r#"data-style="{}""#, style.id)),
                    "{palette}/{style} lost its style",
                );
                assert!(
                    html.contains(&format!(r#"body[data-theme="{palette}"]"#)),
                    "{palette}/{style} has no palette rule to apply",
                );
                assert!(
                    html.contains(&format!(r#"body[data-style="{style}"]"#)),
                    "{palette}/{style} has no style rule to apply",
                );
            }
        }
    }

    #[test]
    fn obs_custom_css_can_choose_both_axes() {
        // The escape hatch. OBS injects its "Custom CSS" box as a `<style>`
        // appended to `<head>` — later in the document than ours, so it wins on
        // source order — and the overlay reads two variables out of it.
        //
        // What this protects is the *reading*, which is the part that can break
        // quietly: injected CSS sets properties, never attributes, so if the
        // overlay stopped copying them onto the body the page would still load,
        // still be legible, and simply ignore everything the streamer asked for.
        let html = overlay_html();

        for expected in [
            // Both axes, under the names the docs promise.
            "'--yap-' + axis",
            "document.body.dataset.theme = theme",
            "document.body.dataset.style = style",
            // And re-applied, because OBS's injection can land after our script
            // has already run — the poll is the retry.
            "applyCssOverrides();",
        ] {
            assert!(
                html.contains(expected),
                "the overlay must contain {expected:?} — without it OBS's Custom \
                 CSS box silently does nothing",
            );
        }

        // An unset variable must leave the attribute alone, or "set one axis and
        // leave the other" would reset the other one on every poll.
        assert!(
            html.contains("if (theme)") && html.contains("if (style)"),
            "each axis must be adopted only when its variable is set",
        );

        // The names the variables use must be the names the tables use, or
        // `--yap-theme: neon` sets an attribute no rule matches and the page
        // keeps whatever the app asked for.
        for palette in OVERLAY_PALETTES {
            assert!(
                html.contains(&format!(r#"body[data-theme="{}"]"#, palette.id)),
                "--yap-theme can only select {}, which has no rule",
                palette.id,
            );
        }
        for style in OVERLAY_STYLES {
            assert!(
                html.contains(&format!(r#"body[data-style="{}"]"#, style.id)),
                "--yap-style can only select {}, which has no rule",
                style.id,
            );
        }
    }

    #[test]
    fn the_css_variables_we_promise_are_the_ones_the_page_uses() {
        // The variables are public API now: a streamer pastes them into OBS and
        // expects them to keep working. Renaming one silently breaks somebody's
        // scene, and nothing else would notice.
        //
        // The list is written out here rather than scraped from the stylesheet on
        // purpose — scraping would check the page against itself and always pass.
        const PROMISED: &[&str] = &[
            "--card",
            "--card-solid",
            "--card-border",
            "--fg",
            "--dim",
            "--accent",
            "--warn",
            "--capped",
            "--radius",
            "--pad-y",
            "--pad-x",
            "--min-width",
            "--count-size",
            "--count-weight",
            "--corner-x",
            "--corner-y",
            "--font",
        ];

        let html = strip_css_comments(overlay_html());

        for variable in PROMISED {
            assert!(
                html.contains(&format!("{variable}:")),
                "{variable} is documented as customisable but the stylesheet never \
                 sets it — setting it in OBS would do nothing",
            );
        }

        // And the documentation has to agree, or we are promising something we
        // never wrote down.
        let docs = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/overlay.md"
        ))
        .expect("the overlay documentation");

        for variable in PROMISED {
            assert!(
                docs.contains(variable),
                "{variable} is customisable and undocumented",
            );
        }
    }

    #[test]
    fn an_old_combined_name_still_shows_what_it_used_to() {
        // A settings.toml written before the split says `overlay_theme = "strip"`.
        // Read as a palette alone, `strip` is not one — and the styles *are*
        // named `strip`, so it would resolve as a shape and hand someone who had
        // never chosen a shape a full-width bar mid-stream.
        //
        // Every old name maps to the pair that reproduces it.
        for old in LEGACY_LOOKS {
            let (palette, style) =
                legacy_look(old).unwrap_or_else(|| panic!("{old} is an old name with no mapping"));

            assert!(
                OVERLAY_PALETTES.iter().any(|p| p.id == palette),
                "{old} maps to palette {palette}, which does not exist",
            );
            assert!(
                OVERLAY_STYLES.iter().any(|s| s.id == style),
                "{old} maps to style {style}, which does not exist",
            );
        }

        // The specific ones that would be actively wrong: `strip` and `big` were
        // shapes, and reading them as palettes gives the wrong shape.
        assert_eq!(legacy_look("strip"), Some(("card", "strip")));
        assert_eq!(legacy_look("big"), Some(("card", "big")));
        assert_eq!(legacy_look("minimal"), Some(("minimal", "slim")));

        // And a name from neither era falls back rather than being guessed at.
        assert_eq!(legacy_look("catppuccin-frappe"), None);
    }

    #[test]
    fn a_theme_that_changes_the_shape_actually_says_so() {
        // The two layout themes are the reason this file has a second kind of
        // theme rule. A `strip` that kept the card's column layout, or a `big`
        // that kept the bar, would satisfy every other test here — it has a
        // rule, it has legible colours, it is offered by the panel — while
        // being the `card` theme under a new name.
        //
        // So check the properties that only the *shape* has: a strip is a row
        // spanning the full width, and a big number has no bar under it.
        // Every style that moves the card has to move it in a *rule*, not in the
        // variable block.
        //
        // This is the third time this has bitten, and the failure is invisible
        // to every other check: the style has a rule, the palette has a rule, the
        // combination renders, the colours pass contrast — it is simply in the
        // wrong corner. Declaring `left` beside the colours loses to the base
        // `.card` rule's `right` on source order, silently.
        //
        // So each style that positions the card is checked for a positioning
        // rule of its own, with the edges it needs cleared.
        for (style, needs) in [
            (
                "slim",
                vec!["left:", "top:", "right: auto;", "bottom: auto;"],
            ),
            (
                "strip",
                vec!["left: 0;", "right: 0;", "bottom: 0;", "top: auto;"],
            ),
            (
                "big",
                vec![
                    "left: 0;",
                    "right: 0;",
                    "top: auto;",
                    "margin-inline: auto;",
                ],
            ),
        ] {
            let rule =
                strip_css_comments(&layout_rule("style", style, &[".card"]).unwrap_or_else(|| {
                    panic!("{style} must position its card in a rule of its own")
                }));

            for property in needs {
                assert!(
                    rule.contains(property),
                    "the {style} style needs {property:?} in its own `.card` rule — \
                     in the variable block it loses to the base rule on source \
                     order and the card stays in the wrong corner. Got: {rule}",
                );
            }
        }

        let strip = layout_rule("style", "strip", &[".card"])
            .expect("strip has a layout rule for the card");
        assert!(
            strip.contains("flex-direction: row;"),
            "strip must lay the card out as a row, not the default column. Got: {strip}",
        );
        assert!(
            strip.contains("left: 0;") && strip.contains("right: 0;"),
            "strip must span the source rather than sit in a corner. Got: {strip}",
        );

        let strip_bar = layout_rule("style", "strip", &[".bar"]).expect("strip lays out its bar");
        assert!(
            strip_bar.contains("flex: 1"),
            "the bar is what fills a strip; everything else keeps its width. Got: {strip_bar}",
        );

        // The centring, which was wrong twice before it was right.
        //
        // Both failures were silent: the theme had a rule, its colours passed
        // the contrast check, and it rendered *something* — just not in the
        // middle. First with `left: 50%` and no pull-back, so the card hung off
        // the right edge; then with `translateX(-50%)`, which put the entire
        // card off-screen and left a blank source. Nothing else in the suite
        // can see either: a theme that is present, legible and in the panel
        // passes every other test here.
        //
        // What makes it centre is layout rather than arithmetic — both edges
        // pinned, with `margin-inline: auto` distributing the slack between
        // them. Asserted as those together, because any one of them alone
        // leaves it visibly off-centre.
        // Comments stripped: the rule's own comment records the `transform` that
        // was tried and removed the card, so a naive search finds the
        // explanation and fails on it.
        let big_card = strip_css_comments(
            &layout_rule("style", "big", &[".card"]).expect("big positions its card"),
        );
        for (property, why) in [
            (
                "margin-inline: auto;",
                "without auto margins nothing is centred at all",
            ),
            ("left: 0;", "an unpinned edge lets the card sit against it"),
            ("right: 0;", "the card needs both edges to span the source"),
            (
                "min-width: 0;",
                "a minimum width beats the free space the margins distribute",
            ),
            (
                "width: fit-content;",
                "otherwise the plate spans the source and only the text is centred",
            ),
        ] {
            assert!(
                big_card.contains(property),
                "big's card needs {property:?} — {why}. Got: {big_card}",
            );
        }

        // And explicitly not the arithmetic version, which is the thing that
        // removed the overlay from the screen entirely.
        assert!(
            !big_card.contains("transform:"),
            "big must not place itself with a transform: one already took the \
             whole card off-canvas, and it promotes a compositing layer that CEF \
             can come back empty. Got: {big_card}",
        );

        // The strip has the mirror-image requirement — full width, pinned to the
        // bottom edge — so the two themes must not be able to drift into each
        // other by accident.
        let strip_card =
            layout_rule("style", "strip", &[".card"]).expect("strip positions its card");
        for property in [
            "left: 0;",
            "right: 0;",
            "bottom: 0;",
            "flex-direction: row;",
        ] {
            assert!(
                strip_card.contains(property),
                "strip's card needs {property:?} to span the source. Got: {strip_card}",
            );
        }
        assert!(
            !strip_card.contains("margin-inline: auto"),
            "strip spans the full width, so centring it would undo the theme. \
             Got: {strip_card}",
        );

        // One grouped rule rather than three, so it is looked up by the
        // selector that leads it.
        let big_hidden = layout_rule("style", "big", &[".status"])
            .expect("big hides its status, bar and row together");
        // The rule was found *by* its selector, so `.status` is what located it.
        // What the body has to say is that the element is hidden.
        assert!(
            big_hidden.contains("display: none;"),
            "big is one number with nothing else. Got: {big_hidden}",
        );

        // And neither may reach for the properties OBS's CEF cannot render —
        // the same list the whole-page test checks, checked here per theme so a
        // new one cannot smuggle one in without that test having to find it.
        //
        // Comments are stripped first. The file explains *why* each property is
        // banned and names them in doing so — this theme's own comment records
        // that a `transform` was tried and removed the card — so a naive
        // substring search finds the explanation and fails on it, which is the
        // kind of check that gets deleted the first time it is inconvenient.
        for theme in OVERLAY_THEMES {
            let rule = strip_css_comments(&theme_rule(theme.id).unwrap_or_default());
            for banned in [
                "filter:",
                "text-shadow",
                "backdrop-filter",
                "mix-blend-mode",
                "clip-path",
            ] {
                assert!(
                    !rule.contains(banned),
                    "{}: {banned:?} is not renderable in OBS",
                    theme.id,
                );
            }
        }
    }

    #[test]
    fn every_theme_is_legible_over_its_own_card() {
        // The whole point of a theme is that it looks good; the one thing it
        // cannot do is become unreadable. Every theme's text and accent are
        // checked against that theme's card colour, at the same 4.5:1 the app
        // window is held to.
        //
        // The `cute` theme is the one this exists for: a pale pastel card with a
        // pale pink number would have looked charming in a mockup and been
        // unreadable on a stream.
        const MINIMUM: f32 = 4.5;

        for theme in OVERLAY_THEMES {
            let block = theme_rule(theme.id).unwrap_or_else(|| panic!("no rule for {}", theme.id));

            let card = hex_of(&block, "--card-solid")
                .unwrap_or_else(|| panic!("{}: no opaque card colour", theme.id));

            for property in ["--fg", "--accent", "--dim"] {
                let colour = hex_of(&block, property)
                    .unwrap_or_else(|| panic!("{}: {property} is not a hex colour", theme.id));
                let ratio = contrast(Rgb::from_hex(&colour), Rgb::from_hex(&card));

                assert!(
                    ratio >= MINIMUM,
                    "{} theme: {property} on the card is {ratio:.2}:1, below {MINIMUM}:1",
                    theme.id,
                );
            }
        }
    }

    /// The body of a theme's CSS rule, without the selector or braces.
    fn theme_rule(id: &str) -> Option<String> {
        let html = overlay_html();
        let start = html.find(&format!(r#"body[data-theme="{id}"] {{"#))?;
        let rest = &html[start..];
        let open = rest.find('{')?;
        let close = rest[open..].find('}')?;
        Some(rest[open + 1..open + close].to_string())
    }

    /// The body of a theme's *layout* rule — the one that positions something,
    /// as opposed to the colour block.
    ///
    /// [`theme_rule`] cannot answer this: it matches `body[data-theme="x"] {`
    /// first, which is the colours, and stops at its closing brace. A layout
    /// rule sits under a longer selector and has to be found by that.
    ///
    /// Matched on *any* of `selectors` rather than an exact selector list,
    /// because a grouped rule is written across several lines and `cargo fmt`
    /// reflows long selector lists — an exact match here would break on a
    /// reformat rather than on a real fault.
    ///
    /// Scoped to the rule that owns the selector, because `flex-direction: row`
    /// and `display: none` appear in more than one: searching the whole page
    /// passes a `strip` that has been quietly reverted to a column by the rule
    /// next door.
    fn layout_rule(axis: &str, id: &str, selectors: &[&str]) -> Option<String> {
        let html = overlay_html();
        // The attribute is a parameter because shape and colour live on different
        // ones now — a `strip` is `body[data-style="strip"]`, and looking for
        // `[data-theme="strip"]` finds nothing and quietly passes.
        let prefix = format!("body[data-{axis}=\"{id}\"]");

        // Every rule on this theme, paired with its selector list.
        //
        // `selector_start` is where `prefix` was found, which is the *start* of
        // the selector rather than the rule's opening brace — so the brace is
        // searched for from there, and `at` advances past the whole rule so the
        // next match is the following one and not this one again.
        let mut at = 0;
        while let Some(found) = html[at..].find(&prefix) {
            let selector_start = at + found;
            let open = selector_start
                + html[selector_start..]
                    .find('{')
                    .expect("a selector is followed by a brace");

            let selector = &html[selector_start..open];
            let body_start = open + 1;
            let close = body_start + html[body_start..].find('}').expect("a rule body is closed");

            // Compared against each selector in a *group*, not the whole list:
            // `body[data-theme="x"] .status, body[data-theme="x"] .bar {…}`
            // reads as one rule but is three selectors, and only the last ends
            // the string.
            //
            // Each is matched on ending with the wanted class rather than
            // containing it, so the colour block — whose selector is exactly
            // `body[data-theme="x"] ` — cannot answer a lookup for `.card` with
            // a rule that positions nothing.
            let grouped = selector.split(',').map(str::trim);
            if grouped
                .clone()
                .any(|part| selectors.iter().any(|want| part.ends_with(want)))
            {
                return Some(html[body_start..close].to_string());
            }

            at = close + 1;
        }

        None
    }

    /// The `#rrggbb` value a custom property is given in a block of CSS.
    fn hex_of(block: &str, property: &str) -> Option<String> {
        let after = block.split(&format!("{property}:")).nth(1)?;
        let value = after.trim_start();

        if !value.starts_with('#') {
            return None;
        }

        Some(value.chars().take(7).collect())
    }

    /// A flat RGB colour, from `#rrggbb`.
    struct Rgb(f32, f32, f32);

    impl Rgb {
        fn from_hex(value: &str) -> Self {
            let value = value.trim_start_matches('#');
            let byte = |index: usize| {
                u8::from_str_radix(&value[index..index + 2], 16).unwrap_or(0) as f32 / 255.0
            };
            Self(byte(0), byte(2), byte(4))
        }
    }

    fn luminance(colour: &Rgb) -> f32 {
        let channel = |value: f32| {
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(colour.0) + 0.7152 * channel(colour.1) + 0.0722 * channel(colour.2)
    }

    fn contrast(a: Rgb, b: Rgb) -> f32 {
        let (first, second) = (luminance(&a), luminance(&b));
        let (lighter, darker) = if first > second {
            (first, second)
        } else {
            (second, first)
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn the_overlay_card_has_a_visible_background() {
        // With no blur to fall back on, the card's background is the only thing
        // making it visible. It has to be opaque enough to read over a video.
        //
        // The card reads its colour from `--card`, so this checks both halves:
        // that the element actually uses the variable, and that every theme
        // gives it something solid. A theme with a transparent card would render
        // as bare text over the stream, which is unreadable most of the time.
        let html = overlay_html();
        assert!(
            html.contains("background: var(--card);"),
            "the card must paint from --card, or theming cannot reach it",
        );

        for theme in OVERLAY_THEMES {
            let block = theme_rule(theme.id).expect("a rule");
            assert!(
                hex_of(&block, "--card").is_some() || block.contains("--card: rgba("),
                "{}: --card must not be transparent",
                theme.id,
            );
            assert!(
                hex_of(&block, "--card-solid").is_some(),
                "{}: no opaque card, so ?opaque cannot work for it",
                theme.id,
            );
        }
    }

    #[test]
    fn the_opaque_variant_has_no_alpha() {
        // For a GPU path where alpha composites to nothing. The body's
        // transparency is untouched — that is what shows the stream through.
        let opaque = opaque_overlay_html(None);

        // The card paints from `--card-solid` in this variant, and every theme
        // supplies a fully opaque value for it.
        assert!(opaque.contains("background: var(--card-solid);"));

        for theme in OVERLAY_THEMES {
            let themed = opaque_overlay_html(Some(theme.id));
            assert!(
                themed.contains("background: var(--card-solid);"),
                "{}: ?opaque did not take effect",
                theme.id,
            );
        }
        // The translucent value still exists as an unused declaration, which is
        // fine; what must not survive is anything *painting* from it.
        assert!(
            !opaque.contains("background: var(--card);"),
            "the card must not paint from the translucent variable in the opaque variant"
        );
        assert!(
            opaque.contains("background: transparent"),
            "the body must stay transparent so the stream shows through"
        );
    }

    #[test]
    fn the_debug_page_paints_with_nothing_the_overlay_depends_on() {
        // A diagnostic that shares the overlay's failure modes is useless, so
        // this uses only `background` and default text rendering.
        let debug = debug_html();

        assert!(
            debug.contains("#ff00ff"),
            "must paint an unmistakable magenta"
        );
        assert!(!debug.contains("backdrop-filter"));
        assert!(!debug.contains("filter:"));
        assert!(!debug.contains("position: absolute"));

        // And it must report the two things that decide the next step.
        assert!(
            debug.contains("/api/state"),
            "must check the server is reachable"
        );
        assert!(
            debug.contains("innerWidth"),
            "must report the viewport size"
        );
        assert!(debug.contains("CEF"), "must reveal whether this is OBS");
    }

    #[test]
    fn the_overlay_is_transparent_over_the_stream() {
        // A browser source with an opaque background hides the stream.
        assert!(overlay_html().contains("background: transparent"));
        assert!(
            overlay_html().contains("pointer-events: none"),
            "the overlay must not swallow clicks meant for anything behind it"
        );
    }

    #[test]
    fn the_overlay_tabs_its_numbers() {
        // Words tick up constantly; a proportional font makes them jitter, which
        // reads as the counter being unstable.
        assert!(overlay_html().contains("font-variant-numeric: tabular-nums"));
    }

    #[test]
    fn the_overlay_handles_a_time_run() {
        // A time run shows a clock, not a word total.
        assert!(overlay_html().contains("'time'"));
        assert!(overlay_html().contains("left"));
    }

    #[test]
    fn the_overlay_shows_the_hard_limit() {
        assert!(overlay_html().contains("goal capped"));
    }

    #[test]
    fn the_overlay_reconnects() {
        // OBS's CEF will not reconnect on its own, and a frozen overlay for the
        // rest of a stream is worse than no overlay.
        assert!(overlay_html().contains("socket.onclose"));
        assert!(overlay_html().contains("setTimeout(connect"));
    }

    #[test]
    fn the_overlay_fades_its_event_line() {
        assert!(overlay_html().contains("6000"));
    }

    #[test]
    fn the_dock_offers_every_mode() {
        for mode in ["fixed", "extend", "time"] {
            assert!(dock_html().contains(mode), "missing mode {mode}");
        }
    }

    #[test]
    fn the_dock_has_the_controls_a_streamer_needs() {
        // Start, pause, resume, undo, end, adjust: the whole lifecycle plus the
        // two escapes (undo for a recognition error, adjust for a stretch the
        // recogniser missed).
        for action in ["pause", "resume", "undo", "end", "adjust"] {
            assert!(dock_html().contains(action), "missing control {action}");
        }
    }

    #[test]
    fn the_dock_shows_the_transcript() {
        // The audit trail is the answer to "you did not say that many words".
        assert!(dock_html().contains("/api/transcript"));
        assert!(dock_html().contains("voided"));
    }

    #[test]
    fn both_pages_are_valid_html_shells() {
        for (name, html) in [("overlay", overlay_html()), ("dock", dock_html())] {
            assert!(
                html.starts_with("<!doctype html>"),
                "{name} is not an html document"
            );
            assert!(html.contains("</html>"), "{name} is unclosed");
        }
    }
}
