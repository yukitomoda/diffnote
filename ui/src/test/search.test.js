// Searching a revision: where a text is, in the files' names, their lines
// (each on its side), and the comments; in the order they are gone to.
import test from 'node:test';
import assert from 'node:assert/strict';
import { SEARCH_MOST, carriedGapLines, excerpt, lineText, occurrences, searchRevision } from '../search.ts';

const q = (text, caseSensitive = false) => ({ text, caseSensitive });
const row = (k, o, n, text) => ({ k, o, n, t: [['kw', text.slice(0, 2)], text.slice(2)] });
const file = (path, rows) => ({ path, status: 'modified', hunks: [{ header: '@@', rows }] });
const doc = (text) => [{ t: 'p', c: [text] }];
const thread = (id, ...comments) => ({ id, resolved: false, comments });
const comment = (id, text, extra = {}) => Object.assign({ id, author: 'a', at: '', doc: doc(text) }, extra);

test('a text is found each time it is in a line, and only as written when the case counts', () => {
  assert.deepEqual(occurrences('Login login LOGIN', q('login')), [[0, 5], [6, 11], [12, 17]]);
  assert.deepEqual(occurrences('Login login LOGIN', q('login', true)), [[6, 11]]);
  assert.deepEqual(occurrences('aaaa', q('aa')), [[0, 2], [2, 4]], 'one after another, not one inside another');
  assert.deepEqual(occurrences('anything', q('')), []);
  assert.deepEqual(occurrences('ログインする', q('グイ')), [[1, 3]]);
});

test('a line is its pieces, as they are drawn', () => {
  assert.equal(lineText([['kw', 'let'], ' x = ', ['str', '"a"'], ';']), 'let x = "a";');
});

test("the files are searched in order: each one's name, then its lines, each on its side", () => {
  const files = [
    file('src/login.ts', [
      row('c', 1, 1, 'const login = 1;'),
      row('d', 2, null, 'old login();'),
      row('a', null, 2, 'new login();'),
    ]),
    file('src/other.ts', [row('a', null, 5, 'nothing here')]),
    file('docs/login.md', []),
  ];
  const r = searchRevision(files, [], q('login'));
  assert.deepEqual(r.files.map((g) => [g.path, !!g.name, g.lines.map((h) => (h.side === 'old' ? '-' : '+') + h.line)]),
    [['src/login.ts', true, ['+1', '-2', '+2']], ['docs/login.md', true, []]], 'a file with nothing found is left out');
  assert.deepEqual(r.all.map((h) => h.kind), ['name', 'line', 'line', 'line', 'name']);
  const first = r.files[0].lines[0];
  assert.deepEqual([first.text, first.span, first.nth], ['const login = 1;', [6, 11], 0]);
  assert.equal(r.capped, false);
});

test('the comments are searched after the files, a deleted one not at all', () => {
  const threads = [
    thread('t1', comment('c1', 'login を確認'), comment('c2', 'login, login')),
    thread('t2', comment('c3', '', { deleted: true }), comment('c4', 'nothing')),
  ];
  const r = searchRevision([file('a.ts', [row('a', null, 1, 'login')])], threads, q('LOGIN'));
  assert.deepEqual(r.comments.map((h) => [h.thread, h.comment, h.nth]), [['t1', 'c1', 0], ['t1', 'c2', 0], ['t1', 'c2', 1]]);
  assert.deepEqual(r.all.map((h) => h.kind), ['line', 'comment', 'comment', 'comment']);
});

test('nothing is searched for an empty text, and too many are cut off', () => {
  assert.deepEqual(searchRevision([file('a.ts', [row('a', null, 1, 'x')])], [], q('')).all, []);
  const many = file('big.ts', Array.from({ length: SEARCH_MOST + 10 }, (_, i) => row('a', null, i + 1, 'xx')));
  const r = searchRevision([many], [], q('x'));
  assert.equal(r.all.length, SEARCH_MOST);
  assert.equal(r.capped, true);
});

test('a line is cut down to what is around what was found', () => {
  assert.deepEqual(excerpt('    let value = login(user);', [16, 21], 16, 80), { head: 'let value = ', found: 'login', tail: '(user);' });
  const long = 'a'.repeat(50) + 'FOUND' + 'b'.repeat(200);
  const e = excerpt(long, [50, 55], 10, 40);
  assert.equal(e.head, '…' + 'a'.repeat(10));
  assert.equal(e.found, 'FOUND');
  assert.ok(e.tail.endsWith('…') && e.tail.length <= 40);
});

test('the lines the diff leaves out go where the page has them, and a line the diff has is not counted twice', () => {
  const f = file('a.ts', [row('c', 10, 10, 'login ten'), row('d', 11, null, 'old login'), row('a', null, 11, 'new login')]);
  const gaps = [
    { path: 'a.ts', line: 3, text: 'login three' },
    { path: 'a.ts', line: 11, text: 'new login' }, // the diff has it
    { path: 'a.ts', line: 40, text: 'login forty' },
    { path: 'other.ts', line: 1, text: 'login' }, // not a file of the page
  ];
  const r = searchRevision([f], [], q('login'), gaps);
  assert.deepEqual(r.files[0].lines.map((h) => (h.gap ? 'gap ' : '') + (h.side === 'old' ? '-' : '+') + h.line),
    ['gap +3', '+10', '-11', '+11', 'gap +40']);
});

test('an exported page searches what it carries of the lines left out, in pieces or as text', () => {
  const f = Object.assign(file('a.ts', []), { gaps: [{ n: 2, o: 1, w: 1, t: [['x'], [['kw', 'let'], ' y']] }, null, { n: 2, o: 9, w: 9, s: ['nine', 'ten'] }, { n: 3, o: 20, w: 20 }] });
  assert.deepEqual(carriedGapLines([f]).map((g) => g.line + ':' + g.text), ['1:x', '2:let y', '9:nine', '10:ten']);
});

test('asked for all, it finds past the most, and stops when its time is up', () => {
  const many = file('big.ts', Array.from({ length: SEARCH_MOST + 10 }, (_, i) => row('a', null, i + 1, 'xx')));
  const all = searchRevision([many], [], q('x'), undefined, { most: Infinity });
  assert.equal(all.all.length, (SEARCH_MOST + 10) * 2);
  assert.deepEqual([all.capped, all.timedOut], [false, false]);
  const late = searchRevision([many], [], q('x'), undefined, { most: Infinity, until: Date.now() - 1 });
  assert.deepEqual([late.capped, late.timedOut], [true, true]);
  assert.ok(late.all.length < SEARCH_MOST, 'what it had found by then');
});

test("the files the page doesn't show come after its own, by path: each one's name, then its lines", () => {
  const others = {
    lines: [
      { path: 'z/b.ts', line: 9, text: 'login nine' },
      { path: 'z/b.ts', line: 2, text: 'login two' },
      { path: 'a.ts', line: 1, text: 'login' }, // the page shows it: not again
    ],
    names: ['m/login.md', 'a.ts'],
  };
  const r = searchRevision([file('a.ts', [row('a', null, 1, 'login')])], [thread('t', comment('c', 'login'))], q('login'), undefined, undefined, others);
  assert.deepEqual(r.others.map((g) => [g.path, !!g.name, g.lines.map((h) => h.line)]), [['m/login.md', true, []], ['z/b.ts', false, [2, 9]]]);
  assert.deepEqual(r.all.map((h) => h.kind + (h.other ? '*' : '')), ['line', 'name*', 'line*', 'line*', 'comment']);
});
