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
    --bg: #101014;
    --fg: #f4f4f5;
    --dim: #a1a1aa;
    --accent: #4ade80;
    --warn: #fbbf24;
    --capped: #f87171;
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
    right: 2rem;
    bottom: 2rem;
    /* `min-width` alone let the card overflow a narrow browser source and be
       clipped by `overflow: hidden`, which is how an overlay that works in a
       browser ends up invisible in a scene sized 320x180. `max-width` as well
       keeps it inside any source the streamer sizes. */
    min-width: 16rem;
    max-width: min(28rem, calc(100% - 4rem));
    box-sizing: border-box;
    padding: 1.25rem 1.5rem;
    /* Opaque, and no `backdrop-filter`.
       OBS's CEF does not render `backdrop-filter` reliably — it can promote the
       element into a compositing layer that comes back empty, taking the card
       with it, while the same page looks perfect in a real browser. That is
       precisely the failure this file had: correct DOM, nothing on screen in
       OBS. A frosted look is not worth a blank overlay, so there is no blur
       here at all. */
    background: rgba(16, 16, 20, 0.92);
    border-radius: 0.75rem;
    border: 1px solid rgba(255, 255, 255, 0.08);
  }
  .status {
    font-size: 0.75rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--dim);
  }
  .count {
    font-size: 3.5rem;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    line-height: 1.05;
  }
  .target { color: var(--dim); font-weight: 400; font-size: 0.45em; }
  .bar {
    height: 0.375rem;
    background: rgba(255, 255, 255, 0.1);
    border-radius: 999px;
    overflow: hidden;
    margin: 0.75rem 0 0.5rem;
  }
  .fill {
    height: 100%;
    background: var(--accent);
    /* A quarter second: long enough to read as motion, short enough that a
       sub extending the goal is visible as the bar going backwards. */
    transition: width 250ms ease-out;
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
    <div class="bar"><div class="fill" id="fill" style="width:0%"></div></div>
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
    fillEl.style.width = '0%';
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

  fillEl.style.width = Math.round(state.fraction * 100) + '%';

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

// Poll as well as subscribing. Cheap: a few bytes, and the interval only matters
// when the socket is unavailable.
setInterval(() => {
  fetch('/api/state')
    .then((r) => r.json())
    .then((body) => render(body.snapshot))
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
  el('fill').style.width = Math.round(state.fraction * 100) + '%';
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
    State(_state): State<Arc<AppState>>,
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

    let body = if flag(query, "debug") {
        debug_html()
    } else if flag(query, "opaque") {
        opaque_overlay_html()
    } else {
        OVERLAY_HTML.to_string()
    };

    Html(body)
        .into_response()
        .with_header(header::CACHE_CONTROL, "no-store, max-age=0")
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

/// The overlay markup, exposed for tests.
pub fn overlay_html() -> &'static str {
    OVERLAY_HTML
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
fn opaque_overlay_html() -> String {
    OVERLAY_HTML.replace(
        "background: rgba(16, 16, 20, 0.92);",
        "background: #101014;",
    )
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
    fn the_overlay_card_has_a_visible_background() {
        // With no blur to fall back on, the card's background is the only thing
        // making it visible. It has to be opaque enough to read over a video.
        assert!(overlay_html().contains("background: rgba(16, 16, 20, 0.92)"));
    }

    #[test]
    fn the_opaque_variant_has_no_alpha() {
        // For a GPU path where alpha composites to nothing. The body's
        // transparency is untouched — that is what shows the stream through.
        let opaque = opaque_overlay_html();

        assert!(opaque.contains("background: #101014"));
        assert!(
            !opaque.contains("rgba(16, 16, 20, 0.92)"),
            "the card's alpha must be gone in the opaque variant"
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
