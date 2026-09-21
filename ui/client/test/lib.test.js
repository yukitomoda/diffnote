'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const lib = require('../lib.js');

const line = (file, start, end, extra) => Object.assign({ kind: 'line', file, side: 'new', start, end, color: 0 }, extra);

test('a location is the path, with the lines of a thread on lines', () => {
  assert.equal(lib.location(line('src/a.ts', 3, 3)), 'src/a.ts:3');
  assert.equal(lib.location(line('src/a.ts', 10, 13)), 'src/a.ts:10-13');
  assert.equal(lib.location({ kind: 'file', file: 'README.md' }), 'README.md');
  assert.equal(lib.location({ kind: 'point', file: 'a.rs', before: 4, absence: 'deleted', was: [] }), 'a.rs');
  assert.equal(lib.location({ kind: 'unplaced', file: 'a.rs', was: [] }), 'a.rs');
  assert.equal(lib.location({ kind: 'global' }), null);
  assert.equal(lib.location(undefined), null);
});

test('the short location has only the name of the file, and 全体 for the review', () => {
  assert.equal(lib.shortLocation(line('src/auth/login.ts', 10, 13)), 'login.ts:10-13');
  assert.equal(lib.shortLocation(line('src/auth/login.ts', 3, 3)), 'login.ts:3');
  assert.equal(lib.shortLocation({ kind: 'file', file: 'docs/README.md' }), 'README.md');
  assert.equal(lib.shortLocation({ kind: 'global' }), '全体');
  assert.equal(lib.baseName('a'), 'a');
});

test('bars stack one per thread, each 4px further in', () => {
  assert.equal(lib.bars([0]), 'inset 3px 0 0 0 #1f77b4');
  assert.equal(lib.bars([1, 2]), 'inset 3px 0 0 0 #ff7f0e, inset 7px 0 0 0 #9467bd');
  assert.equal(lib.bars([8]), 'inset 3px 0 0 0 #1f77b4', 'the palette wraps');
  assert.equal(lib.bars([]), '');
});

test('coverage puts each thread on the lines of its range, on the side it is on', () => {
  const placements = {
    a: line('f', 2, 3),
    b: line('f', 3, 3, { old_range: [1, 2] }),
    c: line('g', 1, 1),
    d: line('f', 5, 6, { side: 'old' }),
    e: { kind: 'file', file: 'f' },
  };
  const cover = lib.coverage(['a', 'b', 'c', 'd', 'e'], placements, 'f');
  assert.deepEqual(cover.new, { 2: ['a'], 3: ['a', 'b'] });
  assert.deepEqual(cover.old, { 1: ['b'], 2: ['b'], 5: ['d'], 6: ['d'] });
});

test('a row is covered on either of its numbers, each thread once in a row', () => {
  const cover = { new: { 3: ['a', 'b'] }, old: { 2: ['b'], 3: ['a'] } };
  assert.deepEqual(lib.covering(cover, { k: 'c', o: 3, n: 3 }), ['a', 'b', 'a']);
  assert.deepEqual(lib.covering(cover, { k: 'c', o: 2, n: 3 }), ['a', 'b']);
  assert.deepEqual(lib.covering(cover, { k: 'a', n: 3 }), ['a', 'b']);
  assert.deepEqual(lib.covering(cover, { k: 'd', o: 2 }), ['b']);
  assert.deepEqual(lib.covering(cover, { k: 'c', o: 9, n: 9 }), []);
  // The same thread on both numbers of a row counts once.
  assert.deepEqual(lib.covering({ new: { 1: ['a'] }, old: { 1: ['a'] } }, { k: 'c', o: 1, n: 1 }), ['a']);
});

test('cards go after the last line of a thread, or before the point it is at', () => {
  const placements = {
    a: line('f', 2, 4),
    b: line('f', 4, 4),
    c: line('f', 7, 7, { side: 'old' }),
    d: { kind: 'point', file: 'f', before: 10, absence: 'deleted', was: ['x'] },
    e: { kind: 'point', file: 'f', before: 1, absence: 'not-yet', was: [] },
    x: line('g', 4, 4),
  };
  const after = lib.cardsAfter(['a', 'b', 'c', 'd', 'e', 'x'], placements, 'f');
  assert.deepEqual(after, { 'new:4': ['a', 'b'], 'old:7': ['c'], 'new:9': ['d'], 'new:1': ['e'] });
  // A row has its new-side cards first, then its old-side ones.
  assert.deepEqual(lib.cardsOfRow({ 'new:4': ['a'], 'old:3': ['z'] }, { k: 'c', o: 3, n: 4 }), ['a', 'z']);
  assert.deepEqual(lib.cardsOfRow(after, { k: 'a', n: 4 }), ['a', 'b']);
  assert.deepEqual(lib.cardsOfRow(after, { k: 'd', o: 7 }), ['c']);
  assert.deepEqual(lib.cardsOfRow(after, { k: 'c', o: 1, n: 2 }), []);
});

const c = (o, n) => ({ k: 'c', o, n });
const d = (o) => ({ k: 'd', o });
const a = (n) => ({ k: 'a', n });

test('side by side: unchanged rows on both sides, a removed run beside the added run', () => {
  assert.deepEqual(lib.pairRows([c(1, 1), d(2), a(2), c(3, 3)]), [
    { left: c(1, 1), right: c(1, 1) },
    { left: d(2), right: a(2) },
    { left: c(3, 3), right: c(3, 3) },
  ]);
});

test('side by side: the shorter side of a change is left empty', () => {
  assert.deepEqual(lib.pairRows([d(1), d(2), d(3), a(1)]), [
    { left: d(1), right: a(1) },
    { left: d(2), right: null },
    { left: d(3), right: null },
  ]);
  assert.deepEqual(lib.pairRows([d(1), a(1), a(2), a(3)]), [
    { left: d(1), right: a(1) },
    { left: null, right: a(2) },
    { left: null, right: a(3) },
  ]);
  assert.deepEqual(lib.pairRows([a(1), a(2)]), [
    { left: null, right: a(1) },
    { left: null, right: a(2) },
  ]);
  assert.deepEqual(lib.pairRows([d(1), d(2)]), [
    { left: d(1), right: null },
    { left: d(2), right: null },
  ]);
  assert.deepEqual(lib.pairRows([]), []);
});

test('side by side: every row appears once on the side it is on', () => {
  const rows = [c(1, 1), d(2), d(3), a(2), c(4, 3), a(4), a(5), c(5, 6), d(6), c(7, 7)];
  const pairs = lib.pairRows(rows);
  const left = pairs.map((p) => p.left).filter(Boolean);
  const right = pairs.map((p) => p.right).filter(Boolean);
  assert.deepEqual(left.map((r) => r.o), rows.filter((r) => r.o != null).map((r) => r.o));
  assert.deepEqual(right.map((r) => r.n), rows.filter((r) => r.n != null).map((r) => r.n));
});

test('a preview is the first line of the comment as plain text, short', () => {
  assert.equal(lib.preview('<p>hello <code>world</code></p>\n<p>second</p>'), 'hello world');
  assert.equal(lib.preview('<p></p><p>  second </p>'), 'second');
  assert.equal(lib.preview('<p>a &amp; b &lt;c&gt; &quot;d&quot;</p>'), 'a & b <c> "d"');
  assert.equal(lib.preview('<h2>Title</h2><p>x</p>'), 'Title');
  assert.equal(lib.preview('line one<br>line two'), 'line one');
  assert.equal(lib.preview(''), '');
  assert.equal(lib.preview('<p>' + 'あ'.repeat(60) + '</p>'), 'あ'.repeat(48) + '…');
  assert.equal(lib.preview('<p>' + 'あ'.repeat(48) + '</p>'), 'あ'.repeat(48));
});

test('a time is shown as date and minutes in the local zone', () => {
  const d = new Date(2026, 8, 20, 9, 5);
  assert.equal(lib.formatTime(d.toISOString()), '2026-09-20 09:05');
  assert.equal(lib.formatTime('not a time'), 'not a time');
});

test('the threads of a file are those on its lines, the file itself, or listed with it', () => {
  const placements = {
    a: line('f', 1, 1),
    b: { kind: 'file', file: 'f' },
    c: { kind: 'global' },
    d: { kind: 'point', file: 'f', before: 3, absence: 'deleted', was: [] },
    e: { kind: 'unplaced', file: 'f', was: [] },
    x: line('g', 1, 1),
  };
  assert.deepEqual(lib.threadsOfFile(['a', 'b', 'c', 'd', 'e', 'x'], placements, 'f'), ['a', 'b', 'd', 'e']);
});

test('counts say how many threads there are and how many are resolved', () => {
  assert.deepEqual(lib.counts([{ resolved: true }, { resolved: false }, { resolved: true }]), { all: 3, resolved: 2 });
  assert.deepEqual(lib.counts([]), { all: 0, resolved: 0 });
});

test('the lines chosen are counted on each side from the counters before the first row and after the last', () => {
  const file = {
    hunks: [{
      header: '@@ -10,4 +10,4 @@ fn',
      rows: [
        { k: 'c', o: 10, n: 10 },
        { k: 'd', o: 11 },
        { k: 'a', n: 11 },
        { k: 'c', o: 12, n: 12 },
      ],
    }, {
      header: '@@ -40,0 +41,1 @@',
      rows: [{ k: 'a', n: 41 }],
    }],
  };
  const flat = lib.flatRows(file);
  assert.deepEqual(flat.map((r) => [r.oldNext, r.newNext]), [[10, 10], [11, 11], [12, 11], [12, 12], [40, 41]]);
  // One added line: nothing on the old side.
  assert.deepEqual(lib.counters(flat, 2, 2), { base: { start: 12, len: 0 }, head: { start: 11, len: 1 } });
  // The removed and the added line (the drag may go either way).
  assert.deepEqual(lib.counters(flat, 2, 1), { base: { start: 11, len: 1 }, head: { start: 11, len: 1 } });
  assert.equal(lib.chosenLocation('a.rs', lib.counters(flat, 0, 3)), 'a.rs:10-12');
  assert.equal(lib.chosenLocation('a.rs', lib.counters(flat, 1, 1)), 'a.rs:11'.replace('11', '11'));
  assert.equal(lib.chosenLocation('a.rs', lib.counters(flat, 4, 4)), 'a.rs:41');
});
