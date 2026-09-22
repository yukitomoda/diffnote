// Where the page is, and what the address says about it. None of this needs a
// browser: the stores and the two directions between them and an address are
// plain functions, which is most of what the browser tests used to have to
// cover by pressing the back button.
import test from 'node:test';
import assert from 'node:assert/strict';
import { lib } from '../lib.ts';
import {
  against,
  at,
  compareWith,
  current,
  hashOfRoute,
  isSection,
  jumpedTo,
  routeOfHash,
  screen,
  showRevision,
  showRoute,
  showScreen,
  startRoute,
} from '../state/route.ts';

lib.setMessages({});

function fresh() {
  current.set(0);
  screen.set(null);
  against.set(null);
  at.set(null);
}

test('an address says which revision, and the page counts from zero', () => {
  fresh();
  assert.deepEqual(routeOfHash('#rev=2', 3), { rev: 2, screen: null, against: null, at: null });
  showRoute(routeOfHash('#rev=2', 3));
  assert.equal(current.get(), 1, 'the second revision');
  assert.equal(hashOfRoute(), '#rev=2');
});

test('an address the review cannot use is no address at all', () => {
  assert.equal(routeOfHash('', 3), null, 'nothing to go on');
  assert.equal(routeOfHash('#rev=0', 3), null, 'the tabs start at one');
  assert.equal(routeOfHash('#rev=4', 3), null, 'there is no fourth revision');
  assert.equal(routeOfHash('#screen=user', 3), null, 'a screen without a revision');
});

test('a screen is one of the ones there are', () => {
  assert.ok(isSection('attachments'));
  assert.ok(!isSection('nowhere'));
  assert.ok(!isSection(null));
  assert.equal(routeOfHash('#rev=1&screen=nowhere', 1).screen, null, 'and is dropped if not');
});

test('the page starts where the address says, or at the last revision', () => {
  fresh();
  assert.equal(startRoute('#rev=1&screen=settings', 3).rev, 1);
  assert.equal(current.get(), 0);
  assert.equal(screen.get(), 'settings');

  fresh();
  assert.equal(startRoute('', 3), null, 'nothing in the address');
  assert.equal(current.get(), 2, 'the last revision is what a review opens at');
  assert.equal(screen.get(), null);
});

test('everything the page is shown with is in the address, and comes back', () => {
  fresh();
  showRevision(2);
  showScreen('attachments');
  compareWith(1);
  jumpedTo({ kind: 'lines', path: 'src/a.ts', side: 'new', start: 10, end: 13 });
  const hash = hashOfRoute();
  assert.equal(hash, '#rev=3&screen=attachments&against=1&at=lines%3Asrc%2Fa.ts%3AR10-13');

  fresh();
  showRoute(routeOfHash(hash, 3));
  assert.equal(current.get(), 2);
  assert.equal(screen.get(), 'attachments');
  assert.equal(against.get(), 1);
  assert.deepEqual(at.get(), { kind: 'lines', path: 'src/a.ts', side: 'new', start: 10, end: 13 });
});

test('a jump to a file and to a thread survives the same round trip', () => {
  for (const place of [
    { kind: 'file', path: 'docs/README.md' },
    { kind: 'thread', id: '01ABCDEF' },
  ]) {
    fresh();
    showRevision(0);
    jumpedTo(place);
    const route = routeOfHash(hashOfRoute(), 1);
    assert.deepEqual(route.at, place);
  }
});
