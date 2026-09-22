import test from 'node:test';
import assert from 'node:assert/strict';
import { lib } from '../lib.js';

// The handful of messages/ja.yaml keys this test file's assertions rely on
// (these tests exercise lib.js's own formatting, not the wording itself).
lib.setMessages({
  'ui.location.whole_review': '全体',
  'ui.image_alt_fallback': '[画像]',
  'ui.image_markdown': '![画像](diffnote-image:{id})',
});

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
  assert.deepEqual(lib.covering(cover, { k: 'c', o: 3, n: 3 }), ['a', 'b']);
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
  const p = (...c) => ({ t: 'p', c });
  assert.equal(lib.preview([p('hello ', { t: 'code', s: 'world' }), p('second')]), 'hello world');
  assert.equal(lib.preview([{ t: 'p' }, p('  second ')]), 'second');
  assert.equal(lib.preview([p('a & b <c> "d"')]), 'a & b <c> "d"', 'text is text, not HTML');
  assert.equal(lib.preview([{ t: 'h', l: 2, c: ['Title'] }, p('x')]), 'Title');
  assert.equal(lib.preview([p('line one', { t: 'br' }, 'line two')]), 'line one');
  assert.equal(lib.preview([{ t: 'ul', c: [{ t: 'li', c: ['item ', { t: 'em', c: ['one'] }] }] }]), 'item one');
  assert.equal(lib.preview([{ t: 'pre', s: 'let a = 1;\n' }]), 'let a = 1;');
  assert.equal(lib.preview([]), '');
  assert.equal(lib.preview([p('あ'.repeat(60))]), 'あ'.repeat(48) + '…');
  assert.equal(lib.preview([p('あ'.repeat(48))]), 'あ'.repeat(48));
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
  assert.equal(lib.chosenLocation('a.rs', lib.counters(flat, 1, 1)), 'a.rs:L11');
  assert.equal(lib.chosenLocation('a.rs', lib.counters(flat, 4, 4)), 'a.rs:41');
});

test('lines chosen on one side of a side by side view are that side\'s, and the other side\'s only if all are unchanged', () => {
  const file = {
    hunks: [{
      header: '@@ -10,4 +10,4 @@',
      rows: [
        { k: 'c', o: 10, n: 10 },
        { k: 'd', o: 11 },
        { k: 'a', n: 11 },
        { k: 'c', o: 12, n: 12 },
      ],
    }],
  };
  const flat = lib.flatRows(file);
  // The removed line, chosen on the old side: no new lines.
  assert.deepEqual(lib.counters(flat, 1, 1, 'old'), { base: { start: 11, len: 1 }, head: { start: 11, len: 0 } });
  // The added line, chosen on the new side: no old lines.
  assert.deepEqual(lib.counters(flat, 2, 2, 'new'), { base: { start: 12, len: 0 }, head: { start: 11, len: 1 } });
  // From the unchanged line above to the added one, on the new side: the
  // removed line between them is not chosen.
  assert.deepEqual(lib.counters(flat, 0, 2, 'new'), { base: { start: 10, len: 0 }, head: { start: 10, len: 2 } });
  // An unchanged line is on both sides, whichever was pressed.
  assert.deepEqual(lib.counters(flat, 3, 3, 'old'), lib.counters(flat, 3, 3));
  assert.deepEqual(lib.counters(flat, 3, 3, 'new'), { base: { start: 12, len: 1 }, head: { start: 12, len: 1 } });
});

test('of the threads on a line only the shown ones count while resolved ones are hidden', () => {
  const byId = { a: { resolved: true }, b: { resolved: false }, c: { resolved: true } };
  assert.deepEqual(lib.shownIds(['a', 'b'], byId, true), ['b']);
  assert.deepEqual(lib.shownIds(['a', 'b'], byId, false), ['a', 'b']);
  assert.deepEqual(lib.shownIds(['a', 'c'], byId, true), []);
});

test('a thread that covers both the old and the new line of a row is one of the row\'s threads, not two', () => {
  const cover = { new: { 46: ['a', 'b', 'c'] }, old: { 38: ['a', 'b', 'c'] } };
  assert.deepEqual(lib.covering(cover, { o: 38, n: 46 }), ['a', 'b', 'c']);
  assert.deepEqual(lib.covering({ new: { 1: ['a', 'b'] }, old: { 1: ['b', 'c'] } }, { o: 1, n: 1 }), ['a', 'b', 'c']);
});

test('pieces are cut where the changed words begin and end, across pieces and kinds', () => {
  const pieces = [['keyword', 'let'], ' x = ', ['string', '"abc"']];
  // Nothing changed: the pieces as they are.
  assert.deepEqual(lib.markPieces(pieces, []), [['keyword', 'let', false], [null, ' x = ', false], ['string', '"abc"', false]]);
  // A word in the plain piece, and the inside of the string.
  assert.deepEqual(lib.markPieces(pieces, [[4, 5], [9, 12]]), [
    ['keyword', 'let', false],
    [null, ' ', false],
    [null, 'x', true],
    [null, ' = ', false],
    ['string', '"', false],
    ['string', 'abc', true],
    ['string', '"', false],
  ]);
  // A range over several pieces marks each part.
  assert.deepEqual(lib.markPieces(pieces, [[2, 5]]), [
    ['keyword', 'le', false],
    ['keyword', 't', true],
    [null, ' x', true],
    [null, ' = ', false],
    ['string', '"abc"', false],
  ]);
  assert.deepEqual(lib.markPieces([], [[0, 3]]), []);
  // The text is kept whole whatever the ranges.
  const whole = lib.markPieces(pieces, [[1, 2], [3, 4], [10, 20]]).map((p) => p[1]).join('');
  assert.equal(whole, 'let x = "abc"');
});

test('the lines a diff leaves out are a marker between the hunks until they are shown', () => {
  const hunk = (o, n, c) => ({ header: `@@ -${o},${c} +${n},${c} @@`, rows: [{ k: 'c', o, n, t: ['x'] }] });
  const file = {
    hunks: [hunk(7, 7, 3), hunk(27, 27, 3)],
    gaps: [{ n: 6, o: 1, w: 1 }, { n: 13, o: 14, w: 14, x: true }, { n: 7, o: 34, w: 34 }],
  };
  // Nothing shown: a marker before, between and after the hunks.
  let out = lib.withGaps(file, {});
  assert.deepEqual(out.hunks.map((h) => (h.marker ? 'marker:' + h.marker.left : 'hunk')), ['marker:6', 'hunk', 'marker:13', 'hunk', 'marker:7']);
  assert.equal(out.hunks[2].marker.prev && out.hunks[2].marker.next, true);
  assert.equal(out.hunks[0].marker.prev, false, 'nothing before the first');
  assert.equal(out.hunks[4].marker.next, false, 'nothing after the last');
  // A file with no places is left alone.
  assert.equal(lib.withGaps({ hunks: [hunk(1, 1, 1)] }, {}).hunks.length, 1);
  // Part of the middle place shown at both ends: the marker keeps what is left,
  // and the hunk after it, run into by the lines shown, gives up its @@ row.
  const rows = (o, c) => Array.from({ length: c }, (_, i) => ({ k: 'c', o: o + i, n: o + i, t: [] }));
  out = lib.withGaps(file, { 1: { top: rows(14, 5), bottom: rows(30, 3) } });
  assert.deepEqual(out.hunks.map((h) => (h.marker ? 'marker:' + h.marker.left : h.quiet ? 'quiet' : 'hunk')), ['marker:6', 'hunk', 'quiet', 'marker:5', 'quiet', 'quiet', 'marker:7']);
  assert.equal(out.hunks[2].header, '@@ -14,5 +14,5 @@', 'a block has a header, for its line numbers');
  // All of it shown: no marker, and the hunk after gives up its @@ row.
  out = lib.withGaps(file, { 1: { top: rows(14, 13), bottom: [] } });
  assert.deepEqual(out.hunks.map((h) => (h.marker ? 'marker' : h.quiet ? 'quiet' : 'hunk')), ['marker', 'hunk', 'quiet', 'quiet', 'marker']);
  // The rows of a selection can be counted across the blocks.
  const flat = lib.flatRows(out);
  assert.deepEqual(flat.map((f) => f.row.n), [7, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27]);
});

test('a press asks for the next lines of the place from the side it names', () => {
  const g = { n: 50, o: 10, w: 10 };
  const none = { top: [], bottom: [] };
  assert.deepEqual(lib.expandRequest(g, none, 'top'), { offset: 0, count: 20, side: 'top' });
  assert.deepEqual(lib.expandRequest(g, none, 'bottom'), { offset: 30, count: 20, side: 'bottom' });
  assert.deepEqual(lib.expandRequest(g, none, 'all'), { offset: 0, count: 50, side: 'top' });
  const some = { top: new Array(20), bottom: new Array(20) };
  assert.deepEqual(lib.expandRequest(g, some, 'top'), { offset: 20, count: 10, side: 'top' });
  assert.deepEqual(lib.expandRequest(g, some, 'bottom'), { offset: 20, count: 10, side: 'bottom' });
  assert.equal(lib.expandRequest(g, { top: new Array(50), bottom: [] }, 'top'), null);
  assert.deepEqual(lib.gapRows(g, 30, [['a'], ['b']]), [{ k: 'c', o: 40, n: 40, t: ['a'] }, { k: 'c', o: 41, n: 41, t: ['b'] }]);
});

test('what was shown is kept by line number, so it holds when the places change', () => {
  const pieces = (n) => ['line ' + n];
  const revealed = {};
  for (let n = 24; n <= 43; n++) revealed[n] = pieces(n);
  revealed[60] = pieces(60);
  // One place of 24-76 (53 lines): the lines from its start are shown, the lone one is not next to anything.
  let shown = lib.shownFrom([null, { n: 53, o: 24, w: 24 }], revealed);
  assert.equal(shown[1].top.length, 20);
  assert.equal(shown[1].bottom.length, 0);
  assert.deepEqual(shown[1].top[0], { k: 'c', o: 24, n: 24, t: ['line 24'] });
  // A thread has brought a hunk in at 47-53: the place is now two, and the shown lines stay with the first.
  shown = lib.shownFrom([null, { n: 23, o: 24, w: 24 }, { n: 23, o: 54, w: 54 }], revealed);
  assert.equal(shown[1].top.length, 20);
  assert.equal(shown[2].top.length, 0);
  // Lines shown up to the end of a place are its bottom, in order.
  const rev2 = {};
  for (let n = 70; n <= 76; n++) rev2[n] = pieces(n);
  shown = lib.shownFrom([null, { n: 53, o: 24, w: 24 }], rev2);
  assert.deepEqual(shown[1].bottom.map((r) => r.n), [70, 71, 72, 73, 74, 75, 76]);
  // A place shown whole is counted once (top), not twice.
  const all = {};
  for (let n = 24; n <= 76; n++) all[n] = pieces(n);
  shown = lib.shownFrom([null, { n: 53, o: 24, w: 24 }], all);
  assert.equal(shown[1].top.length + shown[1].bottom.length, 53);
});

test('a place in a text is a file of the review with its lines, and nothing else is', () => {
  const has = (p) => p === 'src/a.ts' || p === 'docs/記事.md';
  const ref = (text, path, start, end, side = 'new', rev = null) => ({ text, path, side, start, end, rev });
  assert.deepEqual(lib.lineRefs('see src/a.ts:10-13 here', has, 2), ['see ', ref('src/a.ts:10-13', 'src/a.ts', 10, 13), ' here']);
  assert.deepEqual(lib.lineRefs('src/a.ts:7', has, 2), [ref('src/a.ts:7', 'src/a.ts', 7, 7)]);
  // Something in front of the path (a quote, a bracket) stays outside.
  assert.deepEqual(lib.lineRefs('「src/a.ts:3」を見て', has, 1), ['「', ref('src/a.ts:3', 'src/a.ts', 3, 3), '」を見て']);
  assert.deepEqual(lib.lineRefs('docs/記事.md:2-3。', has, 1), [ref('docs/記事.md:2-3', 'docs/記事.md', 2, 3), '。']);
  // Not files of the review, or not lines.
  assert.deepEqual(lib.lineRefs('at 12:30 on http://x.y:8080/a and other.ts:4', has, 1), ['at 12:30 on http://x.y:8080/a and other.ts:4']);
  assert.deepEqual(lib.lineRefs('src/a.ts:0 src/a.ts:9-3', has, 1), ['src/a.ts:0 src/a.ts:9-3']);
  // Two in a text.
  assert.equal(lib.lineRefs('src/a.ts:1 and src/a.ts:2', has, 1).filter((p) => typeof p !== 'string').length, 2);
  assert.deepEqual(lib.lineRefs('', has, 1), []);
});

test('L is the old side, R or nothing the new, and @ names the revision', () => {
  const has = (p) => p === 'a.ts';
  const ref = (text, start, end, side, rev) => ({ text, path: 'a.ts', side, start, end, rev });
  assert.deepEqual(lib.lineRefs('a.ts:L10-12', has, 3), [ref('a.ts:L10-12', 10, 12, 'old', null)]);
  assert.deepEqual(lib.lineRefs('a.ts:R5', has, 3), [ref('a.ts:R5', 5, 5, 'new', null)]);
  assert.deepEqual(lib.lineRefs('a.ts:5@2', has, 3), [ref('a.ts:5@2', 5, 5, 'new', 2)]);
  assert.deepEqual(lib.lineRefs('a.ts:L4-6@3.', has, 3), [ref('a.ts:L4-6@3', 4, 6, 'old', 3), '.']);
  // A revision that isn't there makes it no place.
  assert.deepEqual(lib.lineRefs('a.ts:5@4 a.ts:5@0', has, 3), ['a.ts:5@4 a.ts:5@0']);
});

test('the location of a thread on removed lines has L, and so has the choice of them', () => {
  const p = { kind: 'line', file: 'a.ts', side: 'old', start: 10, end: 12, color: 0 };
  assert.equal(lib.location(p), 'a.ts:L10-12');
  assert.equal(lib.shortLocation(p), 'a.ts:L10-12');
  assert.equal(lib.chosenLocation('a.ts', { base: { start: 4, len: 2 }, head: { start: 4, len: 0 } }), 'a.ts:L4-5');
  assert.equal(lib.chosenLocation('a.ts', { base: { start: 4, len: 2 }, head: { start: 4, len: 3 } }), 'a.ts:4-6');
});

test('lines changed only in white space read as unchanged, keeping both line numbers', () => {
  const row = (k, o, n, text) => ({ k, o, n, t: [text] });
  const file = {
    path: 'a.py',
    hunks: [{
      header: '@@',
      rows: [
        row('c', 1, 1, 'def f():'),
        row('d', 2, undefined, '  a = 1'),
        row('d', 3, undefined, '  b = 2'),
        row('d', 4, undefined, '  c = 3'),
        row('a', undefined, 2, '    a = 1'),
        row('a', undefined, 3, '    b = 3'),
        row('a', undefined, 4, '    c   =   3'),
        row('a', undefined, 5, '    d = 4'),
      ],
    }],
  };
  const out = lib.withoutSpaceChanges(file).hunks[0].rows;
  const shape = out.map((r) => `${r.k}${r.o || ''}:${r.n || ''}`);
  // a and c are unchanged; b is a change; d only added. Numbers only go up.
  assert.deepEqual(shape, ['c1:1', 'c2:2', 'd3:', 'a:3', 'c4:4', 'a:5']);
  // The unchanged row shows the new text.
  assert.deepEqual(out[1].t, ['    a = 1']);
  // Nothing to hide: the same file back.
  const plain = { path: 'b', hunks: [{ header: '@@', rows: [row('d', 1, undefined, 'x'), row('a', undefined, 1, 'y')] }] };
  assert.equal(lib.withoutSpaceChanges(plain), plain);
  // Pieces with kinds count by their text.
  const kinds = { hunks: [{ header: '@@', rows: [{ k: 'd', o: 1, t: [['kw', 'let'], ' x']}, { k: 'a', n: 1, t: [['kw', 'let'], '  x']}] }] };
  assert.deepEqual(lib.withoutSpaceChanges(kinds).hunks[0].rows.map((r) => r.k), ['c']);
});

test('a size is said roughly, and a picture is put in a text where the cursor was', () => {
  assert.equal(lib.formatSize(512), '512 B');
  assert.equal(lib.formatSize(2048), '2 KB');
  assert.equal(lib.formatSize(3 * 1024 * 1024 + 300 * 1024), '3.3 MB');
  const id = 'a'.repeat(64);
  assert.equal(lib.imageMarkdown(id), `![画像](diffnote-image:${id})`);
  assert.deepEqual(lib.insertAt('abcdef', 2, 4, 'XY'), { text: 'abXYef', cursor: 4 });
  assert.deepEqual(lib.insertAt('abc', 3, 3, '!'), { text: 'abc!', cursor: 4 });
  assert.deepEqual(lib.insertAt('abc', 9, 1, '!'), { text: 'abc!', cursor: 4 });
  // An image in a comment is its alt text where only text is wanted.
  assert.equal(lib.plainText([{ t: 'p', c: ['see ', { t: 'image', id, alt: 'a shot' }] }]).trim(), 'see a shot');
  assert.equal(lib.preview([{ t: 'p', c: [{ t: 'image', id, alt: '' }] }]), '[画像]');
});

test('a link to another attached file is named by the file, without what would end the name early', () => {
  const id = 'c'.repeat(64);
  assert.equal(lib.fileMarkdown('report.pdf', id), `[report.pdf](diffnote-file:${id})`);
  assert.equal(lib.fileMarkdown('a[1]\\b.txt', id), `[a1b.txt](diffnote-file:${id})`);
});

test('a limit is shown in megabytes and read back from what was typed', () => {
  assert.equal(lib.bytesToMB(5 * 1024 * 1024), 5);
  assert.equal(lib.bytesToMB(2.5 * 1024 * 1024), 2.5);
  assert.equal(lib.mbToBytes('5'), 5 * 1024 * 1024);
  assert.equal(lib.mbToBytes(' 2,5 '), Math.round(2.5 * 1024 * 1024), 'a comma is a point too');
  for (const bad of ['', '  ', 'abc', '0', '-1', 'Infinity']) assert.equal(lib.mbToBytes(bad), null, bad);
});

test('a text is quoted line by line, and put after what is already written', () => {
  assert.equal(lib.quoteMarkdown('one'), '> one');
  assert.equal(lib.quoteMarkdown('one\ntwo'), '> one\n> two');
  assert.equal(lib.quoteMarkdown('one\n\ntwo\r\nthree'), '> one\n>\n> two\n> three', 'a blank line stays a quotation');
  assert.equal(lib.quoteMarkdown('\n  padded  \n\n'), '>   padded', 'the ends are trimmed, the inside is kept');
  assert.equal(lib.quoteMarkdown('  \n '), '');
  assert.equal(lib.appendQuote('', 'q'), '> q\n\n');
  assert.equal(lib.appendQuote('hello\n', 'q'), 'hello\n\n> q\n\n');
  assert.equal(lib.appendQuote('> a\n\nmine', 'b'), '> a\n\nmine\n\n> b\n\n');
  assert.equal(lib.appendQuote('keep', ' \n '), 'keep', 'nothing to quote: nothing is changed');
});

const EMOJI = [
  ['👍', '+1', 'いいね 賛成 ok'],
  ['🐛', 'bug', 'バグ 虫 不具合'],
  ['🐞', 'lady_beetle', 'バグ 虫'],
  ['🎉', 'tada', 'おめでとう お祝い'],
];

test('an emoji is found by its code, a word or itself, and the codes that begin with the search come first', () => {
  assert.equal(lib.findEmoji(EMOJI, '').length, 4);
  assert.deepEqual(lib.findEmoji(EMOJI, 'bug').map((e) => e[0]), ['🐛']);
  assert.deepEqual(lib.findEmoji(EMOJI, ':bug:').map((e) => e[0]), ['🐛'], 'with the colons');
  assert.deepEqual(lib.findEmoji(EMOJI, 'バグ').map((e) => e[0]), ['🐛', '🐞'], 'by a word');
  assert.deepEqual(lib.findEmoji(EMOJI, '+1').map((e) => e[0]), ['👍']);
  assert.deepEqual(lib.findEmoji(EMOJI, 'BUG').map((e) => e[0]), ['🐛'], 'upper case too');
  assert.deepEqual(lib.findEmoji(EMOJI, '🎉').map((e) => e[0]), ['🎉'], 'the emoji itself');
  assert.deepEqual(lib.findEmoji(EMOJI, 'la').map((e) => e[0]), ['🐞'], 'a part of a code');
  assert.deepEqual(lib.findEmoji(EMOJI, 'zzz'), []);
  // A code that begins with it is before one that only has it in a word.
  const list = [['A', 'alpha', 'beta'], ['B', 'beta', '']];
  assert.deepEqual(lib.findEmoji(list, 'beta').map((e) => e[0]), ['B', 'A']);
});

test('a shortcode is written as its emoji and what is not one is left as it is', () => {
  assert.equal(lib.withShortcodes(EMOJI, 'ok :+1: and :bug:'), 'ok 👍 and 🐛');
  assert.equal(lib.withShortcodes(EMOJI, ':tada::tada:'), '🎉🎉');
  assert.equal(lib.withShortcodes(EMOJI, 'at 12:30:45 and :nope: and http://x:80:'), 'at 12:30:45 and :nope: and http://x:80:');
  assert.equal(lib.withShortcodes(EMOJI, 'no colon'), 'no colon');
  assert.equal(lib.withShortcodes(EMOJI, ':BUG:'), ':BUG:', 'codes are lower case');
  assert.equal(lib.withShortcodes(EMOJI, ':constructor:'), ':constructor:', 'nothing from the object itself');
});

test('a file says how many lines its diff adds and removes, and shows five blocks in that proportion', () => {
  const rows = (kinds) => kinds.split('').map((k) => ({ k }));
  const file = { hunks: [{ rows: rows('ccaadc') }, { rows: rows('dd') }] };
  assert.deepEqual(lib.diffStat(file), { added: 2, removed: 3 });
  assert.deepEqual(lib.diffStat({ hunks: [] }), { added: 0, removed: 0 });
  assert.deepEqual(lib.diffStat({}), { added: 0, removed: 0 });
  assert.deepEqual(lib.diffBlocks(17, 11), ['a', 'a', 'a', 'd', 'd']);
  assert.deepEqual(lib.diffBlocks(10, 0), ['a', 'a', 'a', 'a', 'a']);
  assert.deepEqual(lib.diffBlocks(0, 4), ['d', 'd', 'd', 'd', 'd']);
  assert.deepEqual(lib.diffBlocks(0, 0), ['n', 'n', 'n', 'n', 'n']);
  // A side with a few lines still shows.
  assert.deepEqual(lib.diffBlocks(100, 1), ['a', 'a', 'a', 'a', 'd']);
  assert.deepEqual(lib.diffBlocks(1, 100), ['a', 'd', 'd', 'd', 'd']);
  for (const [a, r] of [[1, 1], [3, 7], [50, 50], [2, 1]]) assert.equal(lib.diffBlocks(a, r).length, 5);
});

test('the page state round-trips through a hash: revision, screen, compare target, and a jump', () => {
  const round = (state) => lib.parseHash(lib.formatHash(state));
  assert.deepEqual(round({ rev: 2, screen: null, against: null, at: null }), { rev: 2, screen: null, against: null, at: null });
  assert.deepEqual(round({ rev: 0, screen: 'general', against: null, at: null }), { rev: 0, screen: 'general', against: null, at: null });
  assert.deepEqual(round({ rev: 3, screen: 'user', against: 1, at: null }), { rev: 3, screen: 'user', against: 1, at: null });
  assert.deepEqual(
    round({ rev: 1, screen: null, against: null, at: { kind: 'file', path: 'src/a b.ts' } }),
    { rev: 1, screen: null, against: null, at: { kind: 'file', path: 'src/a b.ts' } }
  );
  assert.deepEqual(
    round({ rev: 1, screen: null, against: null, at: { kind: 'thread', id: '01ABCXYZ' } }),
    { rev: 1, screen: null, against: null, at: { kind: 'thread', id: '01ABCXYZ' } }
  );
  assert.deepEqual(
    round({ rev: 1, screen: null, against: null, at: { kind: 'lines', path: 'a/b.rs', side: 'new', start: 10, end: 13 } }),
    { rev: 1, screen: null, against: null, at: { kind: 'lines', path: 'a/b.rs', side: 'new', start: 10, end: 13 } }
  );
  assert.deepEqual(
    round({ rev: 1, screen: null, against: null, at: { kind: 'lines', path: 'a/b.rs', side: 'old', start: 10, end: 10 } }),
    { rev: 1, screen: null, against: null, at: { kind: 'lines', path: 'a/b.rs', side: 'old', start: 10, end: 10 } }
  );
});

test('a hash with no revision (or none at all) parses to null: nothing to go on', () => {
  assert.equal(lib.parseHash(''), null);
  assert.equal(lib.parseHash('#'), null);
  assert.equal(lib.parseHash('screen=user'), null);
  assert.equal(lib.parseHash('rev=abc'), null);
  assert.equal(lib.parseHash('#rev-2'), null, 'the old, one-way format is not read back');
  // An unknown screen or a broken `at` is dropped, not fatal.
  assert.deepEqual(lib.parseHash('rev=1&screen=nope&at=garbled'), { rev: 1, screen: null, against: null, at: null });
});

test('the hash is built in a fixed, readable order', () => {
  assert.equal(lib.formatHash({ rev: 1, screen: null, against: null, at: null }), 'rev=1');
  assert.equal(
    lib.formatHash({ rev: 1, screen: 'general', against: 0, at: { kind: 'file', path: 'a.rs' } }),
    'rev=1&screen=general&against=0&at=file%3Aa.rs'
  );
});
