import test from 'node:test';
import assert from 'node:assert/strict';
import { chosenLayout, layout, sidebarBounds, wide } from '../state/view.ts';

test('two columns need the room; one column, of both sides or the new side alone, does not', () => {
  wide.set(true);
  for (const kind of ['unified', 'split', 'new']) {
    chosenLayout.set(kind);
    assert.equal(layout.get(), kind);
  }
  wide.set(false);
  chosenLayout.set('split');
  assert.equal(layout.get(), 'unified', 'no room for two columns');
  chosenLayout.set('new');
  assert.equal(layout.get(), 'new', 'the new side alone is one column');
});

test('the left pane is at least 180px, and at most 1000px or half the window', () => {
  assert.equal(sidebarBounds(50, 1500), 180);
  assert.equal(sidebarBounds(400, 1500), 400);
  assert.equal(sidebarBounds(2000, 1500), 750, 'half the window, where that is less');
  assert.equal(sidebarBounds(2000, 3000), 1000, 'no more than 1000px, however wide the window');
  assert.equal(sidebarBounds(999.6, 4000), 1000);
  assert.equal(sidebarBounds(300, 200), 180, 'a tiny window still leaves the least');
});
