// The files marked as looked at: by revision, by what the file is, reaching
// the revisions after while the file is the same, and kept where the page
// keeps them (the served page's server), or only here.
import test from 'node:test';
import assert from 'node:assert/strict';
import { isViewed, keepWith, marksOf, seen, toggleViewed } from '../state/viewed.ts';

const file = (path, sig) => ({ path, sig, status: 'modified', hunks: [] });
const settled = () => new Promise((resolve) => setImmediate(resolve));
const fresh = () => seen.set({ order: ['r1', 'r2', 'r3'], by: {} });
const viewed = (f, rev) => isViewed(f, seen.get(), rev);

test('a mark is of the file as it was', () => {
  keepWith(null);
  fresh();
  toggleViewed(file('a.rs', 'o|n'), 'r1');
  assert.ok(viewed(file('a.rs', 'o|n'), 'r1'));
  assert.ok(!viewed(file('a.rs', 'o|m'), 'r1'), 'a file that is another one now is not marked');
  toggleViewed(file('a.rs', 'o|n'), 'r1');
  assert.deepEqual(seen.get().by, {}, 'taken back with nothing before it, nothing is left');
});

test('a mark reaches the revisions after, while the file is the same', () => {
  keepWith(null);
  fresh();
  toggleViewed(file('a.rs', 'o|n'), 'r2');
  assert.ok(!viewed(file('a.rs', 'o|n'), 'r1'), 'not back to an earlier one');
  assert.ok(viewed(file('a.rs', 'o|n'), 'r3'), 'the same file after');
  assert.ok(!viewed(file('a.rs', 'o|m'), 'r3'), 'not a file that has changed');
  // Taken back after: said so there, and that reaches on from there.
  toggleViewed(file('a.rs', 'o|n'), 'r3');
  assert.equal(seen.get().by.r3['a.rs'], null);
  assert.ok(viewed(file('a.rs', 'o|n'), 'r2'), 'the earlier mark is left as it was');
  assert.ok(!viewed(file('a.rs', 'o|n'), 'r3'));
  // Marked again where it was taken back.
  toggleViewed(file('a.rs', 'o|n'), 'r3');
  assert.ok(viewed(file('a.rs', 'o|n'), 'r3'));
});

test('the marks a model brings are its revisions\', by id, in order', () => {
  const model = {
    revisions: [
      { id: 'r1', files: [], viewed: { 'a.rs': 'o|n' } },
      { id: 'r2', files: [], viewed: { 'a.rs': null } },
      { id: 'r3', files: [], viewed: {} },
    ],
  };
  assert.deepEqual(marksOf(model), { order: ['r1', 'r2', 'r3'], by: { r1: { 'a.rs': 'o|n' }, r2: { 'a.rs': null } } });
  assert.ok(!isViewed(file('a.rs', 'o|n'), marksOf(model), 'r3'), 'taken back in r2');
});

test('a mark is sent to be kept, and taken back here if it was not', async () => {
  const sent = [];
  let answer = { ok: true };
  keepWith((revision, path, sig, isOn) => {
    sent.push([revision, path, sig, isOn]);
    return Promise.resolve(answer);
  });
  fresh();
  toggleViewed(file('a.rs', 'o|n'), 'r1');
  await settled();
  assert.deepEqual(sent, [['r1', 'a.rs', 'o|n', true]]);
  assert.ok(viewed(file('a.rs', 'o|n'), 'r1'));

  answer = { ok: false, error: 'no' };
  toggleViewed(file('a.rs', 'o|n'), 'r2');
  assert.ok(!viewed(file('a.rs', 'o|n'), 'r2'), 'shown at once');
  await settled();
  assert.deepEqual(sent[1], ['r2', 'a.rs', 'o|n', false]);
  assert.ok(viewed(file('a.rs', 'o|n'), 'r2'), 'not kept: as the review has it');
  keepWith(null);
});
