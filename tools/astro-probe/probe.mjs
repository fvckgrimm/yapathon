#!/usr/bin/env node
//
// Astro probe — StreamElements' WebSocket gateway.
//
// Opens **one connection per token**, so a streamer who cross-streams can
// confirm both are handled at the same time. StreamElements' widget simulator
// only produces events for whichever account is currently active, so testing
// them one at a time proves each works but proves nothing about both at once;
// this runs them concurrently and labels every line with which connection it
// came from.
//
// Credentials come from a `.env` file next to this script: one `SE_JWT_<NAME>`
// variable per account. Falls back to `SE_JWT` for a single account.
//
// Also answers, per connection:
//
//   - which **room** Astro resolved, which is how you know the token was copied
//     while switched to the account you thought
//   - which **provider** its events actually carry, which is how you know the
//     label is right
//   - whether two connections are delivering the **same** platform, which would
//     mean one platform is about to be counted twice
//
// Nothing here is wired into the app.
//
// Usage:
//
//   cp .env.example .env    # then fill in
//   node probe.mjs
//
// Requires Node 22+ for the built-in WebSocket. No packages, no install.

import { createWriteStream } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));

// `.env` if it is there. Missing is fine — the environment can carry the same
// variables — so this never fails, it just does less.
try {
  process.loadEnvFile(join(HERE, '.env'));
} catch {
  // No .env. Environment variables still work.
}

const STAMP = new Date().toISOString().replace(/[:.]/g, '-');
const OUT = process.env.SE_OUT ?? `astro-${STAMP}.jsonl`;

const ENDPOINT = 'wss://astro.streamelements.com';

// The one topic used. Everything measured that a yapathon prices is on it.
const TOPIC = 'channel.activities';

// Field names meaning "this event knows how big it was". Matched anywhere in a
// payload, because the shape differs per activity type and guessing one path
// would miss half of them.
const SIZE_KEY =
  /^(amount|bits|amountmicros|amountdisplaystring|price|value|count|viewers|viewercount|giftcount|month|months|tier)$/i;

/** Every configured account, from `SE_JWT_*` variables, in name order. */
function accounts() {
  const found = Object.entries(process.env)
    .filter(([key, value]) => key.startsWith('SE_JWT_') && value && value.trim())
    .map(([key, value]) => ({
      // `SE_JWT_YOUTUBE` -> `youtube`, which is how it is labelled in the log.
      name: key.slice('SE_JWT_'.length).toLowerCase(),
      token: value.trim(),
    }))
    .sort((a, b) => a.name.localeCompare(b.name));

  if (found.length > 0) return found;

  // The single-account form, so this script still works the way it did before.
  const single = process.env.SE_JWT?.trim();
  return single ? [{ name: 'single', token: single }] : [];
}

// --- per-connection state, so the summary can be printed from memory --------

/** @type {Map<string, object>} keyed by the connection's label */
const conns = new Map();

function connState(label, index) {
  let state = conns.get(label);
  if (!state) {
    state = {
      label,
      index,
      room: null,
      subscribed: false,
      events: 0,
      /** Providers actually seen, with counts. */
      providers: new Map(),
      error: null,
    };
    conns.set(label, state);
  }
  return state;
}

// --- event handling ---------------------------------------------------------

const out = createWriteStream(OUT, { flags: 'a' });
let frames = 0;
let stopping = false;

/** Keys seen anywhere in a payload, and the size-bearing ones among them. */
function sizeKeysOf(node) {
  const size = new Set();
  const walk = (n) => {
    if (Array.isArray(n)) {
      n.forEach(walk);
      return;
    }
    if (n && typeof n === 'object') {
      for (const [k, v] of Object.entries(n)) {
        if (SIZE_KEY.test(k) && (typeof v === 'number' || typeof v === 'string')) size.add(k);
        walk(v);
      }
    }
  };
  walk(node);
  return size;
}

function onFrame(state, frame) {
  frames += 1;
  // `conn` is what lets the analyser group events by which connection delivered
  // them — the whole point of running two at once.
  out.write(`${JSON.stringify({ received_at: Date.now(), conn: state.label, frame })}\n`);

  const type = frame?.type;

  if (type === 'welcome') {
    log(`[${state.label}] connected  client=${frame.data?.client_id}`);
    subscribe(state);
    return;
  }

  if (type === 'response') {
    if (frame.error) {
      state.error = `${frame.error}: ${frame.data?.message ?? ''}`;
      log(`[${state.label}] ✗ subscribe failed — ${state.error}`);
    } else {
      state.subscribed = true;
      // The room Astro resolved from the token. Printed because this is how a
      // token copied while switched to the wrong account announces itself: the
      // room will not be the one the label suggests.
      state.room = frame.data?.room ?? state.room;
      log(`[${state.label}] ✓ subscribed  room=${state.room}`);
    }
    return;
  }

  if (type === 'reconnect') {
    const token = frame.data?.reconnect_token;
    log(`[${state.label}] ↻ server draining; reconnecting`);
    if (state.socket) state.socket.close();
    setTimeout(() => connect(state, `?reconnect_token=${encodeURIComponent(token)}`), 500);
    return;
  }

  if (type !== 'message') return;
  if (frame.topic !== TOPIC) return;

  const activity = frame.data?.data;
  if (!activity) return;

  state.events += 1;

  const provider = frame.data?.provider ?? 'unknown';
  state.providers.set(provider, (state.providers.get(provider) ?? 0) + 1);

  const isMock = frame.data?.isMock === true;
  const size = [...sizeKeysOf(activity)];
  const mark = size.length ? `amount=${activity.amount ?? '-'} [${size.join(',')}]` : 'no size field';

  log(
    `[${state.label}] ${isMock ? 'mock  ' : 'EVENT '} ` +
      `${provider}/${frame.data?.type ?? '?'}`.padEnd(28) +
      ` ${(activity.displayName ?? '?').padEnd(18)} ${mark}`,
  );
}

function subscribe(state) {
  const room = process.env.SE_ROOM?.trim();
  state.socket.send(
    JSON.stringify({
      type: 'subscribe',
      nonce: randomUUID(),
      data: {
        topic: TOPIC,
        ...(room ? { room } : {}),
        token: state.token,
        token_type: 'jwt',
      },
    }),
  );
}

// --- connection -------------------------------------------------------------

const attempts = new Map();

function connect(state, query = '') {
  if (stopping || state.done) return;

  state.socket = new WebSocket(`${ENDPOINT}/${query}`);

  state.socket.addEventListener('message', (event) => {
    let frame;
    try {
      frame = JSON.parse(event.data);
    } catch {
      return;
    }
    try {
      onFrame(state, frame);
    } catch (error) {
      log(`[${state.label}] ! handler error: ${error.message}`);
    }
  });

  state.socket.addEventListener('close', (event) => {
    if (stopping || state.done) return;

    if (query.includes('reconnect_token')) {
      log(`[${state.label}] ✗ reconnect token rejected (${event.code}) — subscribing afresh`);
      query = '';
      attempts.set(state.label, 0);
    }

    const n = (attempts.get(state.label) ?? 0) + 1;
    attempts.set(state.label, n);
    const delay = Math.min(1000 * 2 ** (n - 1), 30_000);
    log(`[${state.label}] ✗ closed (${event.code}) — retrying in ${delay}ms`);
    setTimeout(() => connect(state, query), delay);
  });
}

// --- summary ----------------------------------------------------------------

function summarise() {
  const states = [...conns.values()];

  log(`\n${'═'.repeat(92)}`);
  log(`${states.length} connection(s), ${frames} frames`);
  log(`${'═'.repeat(92)}`);
  log(
    `${'CONN'.padEnd(11)}${'LABEL'.padEnd(11)}${'ROOM'.padEnd(26)}` +
      `${'PROVIDERS SEEN'.padEnd(22)}EVENTS`,
  );
  log(`${'─'.repeat(92)}`);

  for (const s of states) {
    const providers = s.providers.size
      ? [...s.providers].map(([p, n]) => `${p}(${n})`).join(' ')
      : '— nothing yet —';
    log(
      `${String(s.index + 1).padEnd(11)}${s.label.padEnd(11)}` +
        `${(s.room ?? '—').slice(0, 25).padEnd(26)}${providers.padEnd(22)}${s.events}`,
    );
  }
  log(`${'─'.repeat(92)}`);

  // Two questions the run was really for.
  const roomOwner = new Map();
  const providerOwner = new Map();
  let problems = 0;

  for (const s of states) {
    if (s.room) {
      const other = roomOwner.get(s.room);
      if (other && other !== s.label) {
        log(`  ! both "${other}" and "${s.label}" resolved to the same room ${s.room}`);
        log(`    that is one account counted twice — check they were copied while switched apart`);
        problems += 1;
      }
      roomOwner.set(s.room, s.label);
    }

    for (const [provider] of s.providers) {
      const other = providerOwner.get(provider);
      if (other && other !== s.label) {
        log(`  ! "${other}" and "${s.label}" are both delivering ${provider} events`);
        log(`    that platform would be counted twice by the app`);
        problems += 1;
      }
      providerOwner.set(provider, s.label);
    }

    if (s.label !== 'single' && s.providers.size && ![...s.providers.keys()].includes(s.label)) {
      log(`  ~ "${s.label}" is delivering ${[...s.providers.keys()].join(', ')} events`);
      log(`    the label is only a label — pricing follows the event — but it means the`);
      log(`    token was probably copied while switched to a different account`);
    }

    if (s.error) {
      log(`  ✗ ${s.label}: ${s.error}`);
      problems += 1;
    }
  }

  if (states.every((s) => s.events === 0)) {
    log('\n  Nothing arrived on any connection.');
    log('  The simulator only fires for one active account at a time — switch');
    log('  StreamElements between simulations and watch every connection keep up.');
  } else if (problems === 0) {
    log('\n  Every platform is delivered by exactly one connection. That is the');
    log('  configuration the app wants: one source per platform, no double counting.');
  }

  log(`\nNow run:  node analyse.mjs ${OUT}\n`);
}

function log(message) {
  console.log(message);
}

process.on('SIGINT', () => {
  stopping = true;
  for (const state of conns.values()) {
    state.done = true;
    state.socket?.close();
  }
  out.end(() => {
    summarise();
    process.exit(0);
  });
});

// --- go ---------------------------------------------------------------------

const configured = accounts();

if (configured.length === 0) {
  console.error('No tokens found.');
  console.error('');
  console.error('Copy .env.example to .env and add one SE_JWT_<NAME> per account,');
  console.error('or set SE_JWT in the environment for a single one.');
  process.exit(2);
}

log(`Astro probe → ${OUT}`);
log(`Node ${process.version}`);
log(`${configured.length} account(s): ${configured.map((a) => a.label).join(', ')}`);
log('');

// Started together rather than in sequence, which is the point: whether two
// connections coexist is exactly what running them one after another cannot show.
for (const [index, account] of configured.entries()) {
  const state = connState(account.name, index);
  state.token = account.token;
  connect(state);
}
