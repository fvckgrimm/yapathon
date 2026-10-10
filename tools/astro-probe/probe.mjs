#!/usr/bin/env node
//
// Astro probe — StreamElements' WebSocket gateway.
//
// Answers one question, the one the whole integration decision turns on:
//
//   do the event payloads carry enough *size* information to price an event
//   by it, or not?
//
// That decides whether StreamElements could price a Super Chat by what was
// actually spent and a cheer by its bits — which is the entire premise of
// yap-a-thon's pricing rule — or whether every event through this gateway
// would have to be a flat grant.
//
// Nothing here is wired into the app. It connects, subscribes, dumps every
// frame verbatim, and gets out of the way.
//
// Usage:
//
//   SE_JWT=<token> node probe.mjs
//   SE_JWT=<token> SE_ROOM=<channelId> node probe.mjs
//
// SE_ROOM is optional — omitting it subscribes to the authenticated channel.
//
// Requires Node 22+ for the built-in WebSocket. No packages, no install.

import { createWriteStream } from 'node:fs';
import { randomUUID } from 'node:crypto';

const JWT = process.env.SE_JWT;
const ROOM = process.env.SE_ROOM ?? '';
const STAMP = new Date().toISOString().replace(/[:.]/g, '-');
const OUT = process.env.SE_OUT ?? `astro-${STAMP}.jsonl`;

const ENDPOINT = 'wss://astro.streamelements.com';

// The four topics that between them could answer the question. `activities` is
// the event feed, `session.update` carries keyed session objects that the
// Session Data Reference documents amounts for, `tips` is donations, and
// `chat.message` is what a `!yap` command would arrive on.
const TOPICS = [
  'channel.activities',
  'channel.session.update',
  'channel.tips',
  'channel.chat.message',
];

// Field names that would mean "this event knows how big it was". Matched
// case-insensitively anywhere in a payload, because the shape differs per topic
// and per activity type and guessing a single path would miss half of them.
const SIZE_KEY = /^(amount|bits|amountmicros|amountdisplaystring|price|value|count|viewers|viewercount|giftcount|month|months|tier)$/i;

if (!JWT) {
  console.error('SE_JWT is required. Copy it from the StreamElements dashboard:');
  console.error('  avatar (top right) > your channel > copy the JWT token.');
  console.error('See README.md in this directory.');
  process.exit(2);
}

const out = createWriteStream(OUT, { flags: 'a' });
const counts = { frames: 0, activities: 0, sessionUpdates: 0, tips: 0, chat: 0 };
let subscribed = 0;
let attempts = 0;
let socket = null;
let stopping = false;

// Everything seen, so the summary at the end can be printed from memory rather
// than by re-reading the file. One entry per interesting frame.
const seen = new Map(); // key -> { topic, kind, sizeKeys:Set, sample }

function log(...parts) {
  console.log(...parts);
}

function record(topic, kind, payload, frame) {
  const sizeKeys = new Set();
  const walk = (node) => {
    if (Array.isArray(node)) {
      node.forEach(walk);
      return;
    }
    if (node && typeof node === 'object') {
      for (const [k, v] of Object.entries(node)) {
        if (SIZE_KEY.test(k) && (typeof v === 'number' || typeof v === 'string')) {
          sizeKeys.add(k);
        }
        walk(v);
      }
    }
  };
  walk(payload);

  const key = `${topic}|${kind}`;
  const entry = seen.get(key) ?? { topic, kind, n: 0, sizeKeys: new Set(), sample: frame };
  entry.n += 1;
  sizeKeys.forEach((k) => entry.sizeKeys.add(k));
  seen.set(key, entry);

  const mark = sizeKeys.size ? `[size: ${[...sizeKeys].join(', ')}]` : '[NO SIZE FIELD]';
  return mark;
}

function onFrame(frame) {
  counts.frames += 1;
  out.write(`${JSON.stringify({ received_at: Date.now(), frame })}\n`);

  const type = frame?.type;

  if (type === 'welcome') {
    log(`\n✓ connected  client=${frame.data?.client_id}`);
    subscribeAll();
    return;
  }

  if (type === 'response') {
    if (frame.error) {
      log(`  ✗ subscribe failed: ${frame.error} — ${frame.data?.message}`);
    } else if (subscribed < TOPICS.length) {
      subscribed += 1;
      log(`  ✓ subscribed to ${frame.data?.topic}  room=${frame.data?.room}`);
    }
    return;
  }

  if (type === 'reconnect') {
    // Graceful drain. The token restores every subscription on the new
    // connection, so this is the one reconnection that is not a gap.
    const token = frame.data?.reconnect_token;
    log(`\n↻ server draining; reconnecting with token`);
    if (socket) socket.close();
    connect(`?reconnect_token=${encodeURIComponent(token)}`);
    return;
  }

  if (type !== 'message') return;

  const topic = frame.topic;
  const data = frame.data ?? {};

  if (topic === 'channel.activities') {
    counts.activities += 1;
    const kind = `${data.type}/${data.provider}`;
    const mark = record(topic, kind, data.data ?? data, data);
    log(`  activity   ${kind.padEnd(28)} ${data.data?.displayName ?? '?'}  ${mark}`);
    return;
  }

  if (topic === 'channel.session.update') {
    counts.sessionUpdates += 1;
    // The docs say `data.key` and `data.name` are the same value. They are not
    // both present: a real payload carries only `name`, so reading `key` alone
    // labels every session frame "undefined". Falls back to whichever is there.
    const key = data.key ?? data.name ?? '(no key)';
    const mark = record(topic, key, data.data ?? {}, data);
    log(`  session    ${String(key).padEnd(28)} ${data.provider ?? '?'}  ${mark}`);
    return;
  }

  if (topic === 'channel.tips') {
    counts.tips += 1;
    const mark = record(topic, `tip/${data.provider}`, data.donation ?? {}, data);
    const who = data.donation?.user?.username ?? '?';
    log(`  tip        ${who.padEnd(28)} ${data.donation?.currency ?? ''} ${data.donation?.amount ?? '?'}  ${mark}`);
    return;
  }

  if (topic === 'channel.chat.message') {
    counts.chat += 1;
    const mark = record(topic, 'message', data, data);
    const who = data.displayName ?? data.nick ?? '?';
    log(`  chat       ${who.padEnd(28)} ${mark}`);
  }
}

function subscribeAll() {
  for (const topic of TOPICS) {
    socket.send(
      JSON.stringify({
        type: 'subscribe',
        nonce: randomUUID(),
        data: {
          topic,
          ...(ROOM ? { room: ROOM } : {}),
          token: JWT,
          token_type: 'jwt',
        },
      }),
    );
  }
  log(`\n  subscribed to ${TOPICS.length} topics${ROOM ? ` (room ${ROOM})` : ' (default room)'}`);
  log('  now trigger some events — see README.md for the checklist.\n');
}

function connect(query = '') {
  if (stopping) return;

  socket = new WebSocket(`${ENDPOINT}/${query}`);

  socket.addEventListener('message', (event) => {
    let frame;
    try {
      frame = JSON.parse(event.data);
    } catch {
      return;
    }
    try {
      onFrame(frame);
    } catch (error) {
      log(`  ! handler error: ${error.message}`);
    }
  });

  socket.addEventListener('close', (event) => {
    if (stopping) return;

    if (query.includes('reconnect_token')) {
      log(`  ✗ reconnect token rejected (${event.code}) — resubscribing from scratch`);
      subscribed = 0;
      query = '';
    }

    // Exponential backoff, capped. EventSub has the same shape; a yapathon
    // whose chat quietly stops counting has a goal that stops meaning anything.
    attempts += 1;
    const delay = Math.min(1000 * 2 ** (attempts - 1), 30_000);
    log(`  ✗ closed (${event.code}) — retrying in ${delay}ms`);
    setTimeout(() => connect(query), delay);
  });

  socket.addEventListener('error', () => {
    // `close` always follows, and that is where reconnection is handled.
  });
}

function summarise() {
  log(`\n${'─'.repeat(72)}`);
  log(`frames ${counts.frames}  activities ${counts.activities}  session ${counts.sessionUpdates}  tips ${counts.tips}  chat ${counts.chat}`);
  log(`${'─'.repeat(72)}`);
  log(`${'TOPIC'.padEnd(24)} ${'KIND'.padEnd(26)} ${'N'.padStart(3)}  SIZE FIELDS`);
  log(`${'─'.repeat(72)}`);

  for (const entry of seen.values()) {
    const size = entry.sizeKeys.size ? [...entry.sizeKeys].join(', ') : '— none —';
    log(`${entry.topic.padEnd(24)} ${entry.kind.padEnd(26)} ${String(entry.n).padStart(3)}  ${size}`);
  }
  log(`${'─'.repeat(72)}`);
  log(`\nNow run:  node analyse.mjs ${OUT}`);
}

process.on('SIGINT', () => {
  stopping = true;
  if (socket) socket.close();
  out.end(() => {
    summarise();
    process.exit(0);
  });
});

log(`Astro probe → ${OUT}`);
log(`Node ${process.version}`);
connect();
