#!/usr/bin/env node
//
// Reads an Astro probe dump and answers the question the decision turns on:
//
//   for each event kind, does the payload carry a size, and is that size
//   what yap-a-thon's pricing rule needs?
//
// Eyeballing JSON is how a field gets missed. This collects the union of every
// key seen anywhere in each kind's payloads, then checks it against what the
// pricing rule requires — so a field that only appears on the third event of
// its kind still shows up.
//
// Usage:
//
//   node analyse.mjs astro-2026-10-10T12-00-00-000Z.jsonl
//   node analyse.mjs *.jsonl          # compares dumps side by side

import { readFileSync } from 'node:fs';
import { basename } from 'node:path';

// What the pricing rule needs per kind. `amount` present means scale by it,
// absent means flat. The right column is what a flat grant would cost us.
//
// Drawn from the Session Data Reference and the widget event schema, both of
// which document amounts for these kinds — the question is only whether they
// survive the trip over this gateway.
const NEEDED = {
  'cheer': { wants: 'bits', why: 'a flat cheer ignores how much was cheered' },
  'superchat': { wants: 'money amount', why: 'the whole point: $5 should be worth 5x $1' },
  'subscriber': { wants: 'none — flat is right', why: 'one sub is one sub' },
  'sponsor': { wants: 'none — flat is right', why: 'one membership is one membership' },
  'follower': { wants: 'none — flat is right', why: 'one follow is one follow' },
  'communityGiftPurchase': { wants: 'count', why: '50 gifted subs should be worth 50 subs' },
  'raid': { wants: 'viewers', why: 'optional, but streamers really do price per raider' },
  'host': { wants: 'viewers', why: 'optional, same as raid' },
  'tip': { wants: 'money amount', why: 'tips are the reason this gateway is interesting' },
  'channelPointsRedemption': { wants: 'none — flat is right', why: 'a redemption is one event' },
  'redemption': { wants: 'none — flat is right', why: 'same' },
  // YouTube only, and only over this gateway: the native Live Streaming API has
  // no follow notification at all, so this is the one event an Astro channel
  // gets that a native YouTube one never can.
  'subscriber': { wants: 'none — flat is right', why: 'a subscriber is one subscriber' },
  // A YouTube channel membership. Flat is defensible; `amount` is there if
  // tier-weighted pricing is ever wanted.
  'sponsor': { wants: 'none — flat is right, or amount by tier', why: 'membership, not subscription' },
  'merch': { wants: 'price', why: 'a purchase should be worth what was paid' },
  'purchase': { wants: 'amount', why: 'same' },
};

// `channel.session.update` names its payload with Session Data Reference keys
// — `cheer-latest`, `subscriber-gifted-latest` — while `channel.activities`
// names the same things `cheer`, `communityGiftPurchase`. Without folding one
// vocabulary into the other the analyser scored the entire session topic "no
// opinion", which is the topic most likely to carry the amounts we care about.
const SESSION_ALIAS = {
  cheer: 'cheer',
  superchat: 'superchat',
  subscriber: 'subscriber',
  'subscriber-new': 'subscriber',
  'subscriber-resub': 'subscriber',
  'subscriber-gifted': 'communityGiftPurchase',
  sponsor: 'sponsor',
  'sponsor-gifted': 'communityGiftPurchase',
  follower: 'follower',
  // StreamElements uses both spellings; `follow` in activities, `follower` in session keys.
  follow: 'follower',
  raid: 'raid',
  host: 'host',
  tip: 'tip',
  charitycampaigndonation: 'tip',
  kicks: 'tip',
  merch: 'purchase',
  purchase: 'purchase',
  'channel-points': 'channelPointsRedemption',
  redemption: 'redemption',
};

/** The activity name a kind maps onto, so both topics are scored the same. */
function activityFor(kind) {
  const base = kind.split('/')[0];
  const stripped = base.replace(/-(latest|recent)$/, '');

  // Alias first — `subscriber-gifted-latest` means a gift bomb, not a sub — then
  // the stripped name itself, so `communityGiftPurchase-latest` resolves without
  // needing an alias entry for every activity type that already exists.
  return (
    SESSION_ALIAS[stripped.toLowerCase()] ??
    (NEEDED[stripped] ? stripped : base)
  );
}

const SIZE_KEY = /^(amount|bits|amountmicros|amountdisplaystring|price|value|count|viewers|viewercount|giftcount|month|months|tier)$/i;

function classify(file) {
  const lines = readFileSync(file, 'utf8').trim().split('\n').filter(Boolean);

  const kinds = new Map();
  let frames = 0;
  let first = null;
  let last = null;
  let latencies = [];

  for (const line of lines) {
    let row;
    try {
      row = JSON.parse(line);
    } catch {
      continue;
    }
    frames += 1;
    first ??= row.received_at;
    last = row.received_at;

    const frame = row.frame ?? {};
    if (frame.type !== 'message') continue;

    const topic = frame.topic;
    const data = frame.data ?? {};

    let kind;
    let payload;
    if (topic === 'channel.activities') {
      kind = `${data.type}/${data.provider}`;
      payload = data;
    } else if (topic === 'channel.session.update') {
      // Only `name` is present in real payloads, despite the docs listing both.
      const key = data.key ?? data.name ?? '(no key)';
      kind = `${key}/${data.provider ?? '?'}`;
      payload = data.data ?? {};
    } else if (topic === 'channel.tips') {
      kind = `tip/${data.provider}`;
      payload = data.donation ?? {};
    } else {
      kind = topic;
      payload = data;
    }

    const entry = kinds.get(kind) ?? {
      topic, kind, n: 0, keys: new Set(), sizeKeys: new Set(), sample: frame,
    };
    entry.n += 1;

    const walk = (node, path) => {
      if (Array.isArray(node)) {
        node.forEach((v, i) => walk(v, `${path}[${i}]`));
        return;
      }
      if (node && typeof node === 'object') {
        for (const [k, v] of Object.entries(node)) {
          entry.keys.add(k);
          if (SIZE_KEY.test(k) && (typeof v === 'number' || typeof v === 'string')) {
            entry.sizeKeys.add(k);
          }
          walk(v, `${path}.${k}`);
        }
      }
    };
    walk(payload, '');

    kinds.set(kind, entry);

    if (frame.ts) {
      const sent = Date.parse(frame.ts);
      if (!Number.isNaN(sent)) latencies.push(Math.abs(row.received_at - sent));
    }
  }

  latencies.sort((a, b) => a - b);
  const median = latencies.length ? latencies[Math.floor(latencies.length / 2)] : null;

  return { file, frames, first, last, kinds, medianLatency: median, n: latencies.length };
}

function report(r) {
  const mins = r.first && r.last ? ((r.last - r.first) / 60000).toFixed(1) : '0';
  console.log(`\n${'═'.repeat(78)}`);
  console.log(`${basename(r.file)}`);
  console.log(`${r.frames} frames over ${mins} min · median delivery ${r.medianLatency === null ? 'n/a' : `${r.medianLatency}ms`} (n=${r.n})`);

  if (r.kinds.size === 0) {
    console.log('\n  Nothing was captured. Did the stream run? Were you subscribed?\n');
    return;
  }

  console.log(`${'─'.repeat(78)}`);
  console.log(`${'KIND'.padEnd(34)} ${'N'.padStart(3)}  ${'SIZE FIELDS'.padEnd(26)} VERDICT`);
  console.log(`${'─'.repeat(78)}`);

  let unknown = 0;

  for (const entry of [...r.kinds.values()].sort((a, b) => a.topic.localeCompare(b.topic) || a.kind.localeCompare(b.kind))) {
    const base = activityFor(entry.kind);
    const need = NEEDED[base];
    const has = [...entry.sizeKeys].join(', ') || '— none —';

    let verdict;
    if (!need) {
      verdict = 'no opinion';
      unknown += 1;
    } else if (entry.sizeKeys.size === 0 && need.wants.startsWith('none')) {
      verdict = 'ok (flat is right)';
    } else if (entry.sizeKeys.size > 0) {
      verdict = 'HAS SIZE';
    } else {
      verdict = `FLAT ONLY — ${need.why}`;
    }

    console.log(`${entry.kind.padEnd(34)} ${String(entry.n).padStart(3)}  ${has.padEnd(26)} ${verdict}`);
  }

  console.log(`${'─'.repeat(78)}`);

  const priced = [...r.kinds.values()].filter((e) => NEEDED[activityFor(e.kind)] && e.sizeKeys.size > 0).length;
  const flatOnly = [...r.kinds.values()].filter((e) => {
    const need = NEEDED[activityFor(e.kind)];
    return need && e.sizeKeys.size === 0 && !need.wants.startsWith('none');
  });

  if (flatOnly.length) {
    console.log('\nKinds that would be FLAT GRANTS — the pricing rule cannot scale them:');
    for (const e of flatOnly) console.log(`  · ${e.kind}  (${NEEDED[activityFor(e.kind)].why})`);
  }
  if (priced) console.log(`\nKinds carrying a size, so they can be priced proportionally: ${priced}`);
  if (unknown) console.log(`\n${unknown} kind(s) with no stated expectation — worth a look before deciding.`);

  console.log(`\nAll keys seen, per kind (for anything the table missed):`);
  for (const entry of [...r.kinds.values()].sort((a, b) => b.sizeKeys.size - a.sizeKeys.size)) {
    console.log(`  ${entry.kind}`);
    console.log(`    ${[...entry.keys].sort().join(', ')}`);
  }
  console.log('');
}

const files = process.argv.slice(2);
if (files.length === 0) {
  console.error('usage: node analyse.mjs <dump.jsonl> [...]');
  process.exit(2);
}
for (const file of files) report(classify(file));
