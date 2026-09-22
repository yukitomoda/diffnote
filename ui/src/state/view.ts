// How the diff is shown: what the reader prefers, not what the review says.
//
// These belong to the reader and to this browser, not to the review, so they
// are kept where the browser lets us and are the same for every part of the
// page at once -- which is why they are stores rather than state in the app.
// (`ignore_whitespace` starts from the review's own setting, and changing it
// here changes only what this page shows.)
import { atom, computed } from 'nanostores';
import { keep, kept } from './kept.ts';

export type Layout = 'unified' | 'split';

/**
 * The layout that was chosen, which is not always the one shown: two columns
 * need the room for them.
 */
export const chosenLayout = atom<Layout>('unified');

/** Whether there is room for two columns (only read when the page opens). */
export const wide = atom(false);

export const layout = computed([chosenLayout, wide], (chosen, room): Layout =>
  chosen === 'split' && room ? 'split' : 'unified',
);

export const hideResolved = atom(true);
export const ignoreWhitespace = atom(false);

export function setLayout(next: Layout): void {
  keep('diffnote-layout', next);
  chosenLayout.set(next);
}

export function setHideResolved(on: boolean): void {
  keep('diffnote-hide-resolved', on ? '1' : '0');
  hideResolved.set(on);
}

export function setIgnoreWhitespace(on: boolean): void {
  ignoreWhitespace.set(on);
}

/**
 * What the page starts with: what was kept from last time, and, where nothing
 * was, the review's own setting and the width of the window.
 *
 * Only when the page opens: a window made wider afterwards doesn't rearrange
 * the diff under the reader.
 */
export function startView(reviewIgnoresWhitespace: boolean): void {
  const stored = kept('diffnote-layout', '');
  wide.set(window.matchMedia('(min-width: 900px)').matches);
  chosenLayout.set(
    stored === 'split' || stored === 'unified'
      ? stored
      : window.matchMedia('(min-width: 1200px)').matches
        ? 'split'
        : 'unified',
  );
  hideResolved.set(kept('diffnote-hide-resolved', '1') !== '0');
  ignoreWhitespace.set(reviewIgnoresWhitespace);
}

/** Two columns need the room: the page follows the window while it is open. */
export function watchWidth(): () => void {
  const mq = window.matchMedia('(min-width: 900px)');
  const on = () => wide.set(mq.matches);
  on();
  if (mq.addEventListener) mq.addEventListener('change', on);
  else mq.addListener(on);
  return () => {
    if (mq.removeEventListener) mq.removeEventListener('change', on);
    else mq.removeListener(on);
  };
}
