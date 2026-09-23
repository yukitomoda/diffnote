// Where the page is, and what the address says about it. None of this needs a
// browser: the stores and the two directions between them and an address are
// plain functions, which is most of what the browser tests used to have to
// cover by pressing the back button.
import test from 'node:test';
import assert from 'node:assert/strict';
import { lib } from '../lib.ts';
import {
  SECTIONS,
  SECTION_GROUPS,
  compareWith,
  goTo,
  hashOfRoute,
  isSection,
  openRevision,
  route,
  routeOfHash,
  showRevision,
  showRoute,
  showScreen,
} from '../state/route.ts';

lib.setMessages({});

function fresh() {
  route.set({ rev: 0, screen: null, against: null, at: null });
}

/** A jump that stays in the revision being shown. */
function jumpTo(place) {
  goTo(route.get().rev, place);
}

test('an address says which revision, and the page counts from zero', () => {
  fresh();
  assert.deepEqual(routeOfHash('#rev=2', 3), { rev: 2, screen: null, against: null, at: null });
  showRoute(routeOfHash('#rev=2', 3));
  assert.equal(route.get().rev, 1, 'the second revision');
  assert.equal(hashOfRoute(), '#rev=2');
});

test('an address the review cannot use is no address at all', () => {
  assert.equal(routeOfHash('', 3), null, 'nothing to go on');
  assert.equal(routeOfHash('#rev=0', 3), null, 'the tabs start at one');
  assert.equal(routeOfHash('#rev=4', 3), null, 'there is no fourth revision');
  assert.equal(routeOfHash('#screen=user', 3), null, 'a screen without a revision');
});

test('every screen there is has a place in the nav that leads to it', () => {
  // `SECTIONS` says which names an address may carry; `SECTION_GROUPS` says
  // how they are put in front of a reader. A screen in one and not the other
  // is either unreachable or a dead link.
  const listed = SECTION_GROUPS.flat();
  assert.deepEqual([...listed].sort(), [...SECTIONS].sort());
  assert.equal(listed.length, new Set(listed).size, 'and in one place only');
});

test('a screen is one of the ones there are', () => {
  assert.ok(isSection('attachments'));
  assert.ok(!isSection('nowhere'));
  assert.ok(!isSection(null));
  assert.equal(routeOfHash('#rev=1&screen=nowhere', 1).screen, null, 'and is dropped if not');
});

test('a move is one move: a tab leaves no screen or jump behind it', () => {
  fresh();
  showScreen('settings');
  jumpTo({ kind: 'file', path: 'a.rs' });
  openRevision(1);
  assert.deepEqual(route.get(), { rev: 1, screen: null, against: null, at: null });
});

test('a jump shows the review, at the place, in the revision it names', () => {
  fresh();
  showScreen('attachments');
  goTo(2, { kind: 'thread', id: '01ABC' });
  assert.deepEqual(route.get(), { rev: 2, screen: null, against: null, at: { kind: 'thread', id: '01ABC' } });
});

test('everything the page is shown with is in the address, and comes back', () => {
  fresh();
  showRevision(2);
  compareWith(1);
  jumpTo({ kind: 'lines', path: 'src/a.ts', side: 'new', start: 10, end: 13 });
  showScreen('attachments');
  const hash = hashOfRoute();
  assert.equal(hash, '#rev=3&screen=attachments&against=1&at=lines%3Asrc%2Fa.ts%3AR10-13');

  fresh();
  showRoute(routeOfHash(hash, 3));
  assert.equal(route.get().rev, 2);
  assert.equal(route.get().screen, 'attachments');
  assert.equal(route.get().against, 1);
  assert.deepEqual(route.get().at, { kind: 'lines', path: 'src/a.ts', side: 'new', start: 10, end: 13 });
});

test('a jump to a file and to a thread survives the same round trip', () => {
  for (const place of [
    { kind: 'file', path: 'docs/README.md' },
    { kind: 'thread', id: '01ABCDEF' },
  ]) {
    fresh();
    jumpTo(place);
    const route = routeOfHash(hashOfRoute(), 1);
    assert.deepEqual(route.at, place);
  }
});
