// How the diff is shown: what the reader prefers, not what the review says.
//
// These belong to the reader, not to the review, and are the same for every
// part of the page at once -- which is why they are stores rather than state
// in the app. The served page keeps them in this machine's user settings,
// through its server: its address has a new port every run, and what the
// browser keeps is kept per address. An exported page has no server, and
// keeps them in the browser.
// (`ignore_whitespace` starts from the review's own setting, and changing it
// here changes only what this page shows.)
import { atom, computed } from 'nanostores';
import { keep, kept } from './kept.ts';
import { transport } from '../transport.ts';
import type { ViewPrefs } from '../model.ts';

/** Keeps a choice: with the user settings where there is a server, and in
 * the browser always (for an exported page, and as what the user settings
 * fall back to before anything is in them). */
function remember(key: string, value: string, pref: ViewPrefs): void {
  keep(key, value);
  if (transport) transport.post('/api/view', pref).catch(function () { /* kept for this page only */ });
}

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
/** Whether a line too long for its column goes on to the next row. */
export const wrapLines = atom(true);
/** Whether, unwrapped and side by side, the two sides scroll as one. */
export const syncScroll = atom(true);
export const ignoreWhitespace = atom(false);

/** How wide the left pane is (px), and the bounds of what it may be. */
export const SIDEBAR_DEFAULT = 280;
export const SIDEBAR_MIN = 180;
export const sidebarWidth = atom(SIDEBAR_DEFAULT);

/** The width, kept inside the bounds: not under the least, and never so
 * wide the diff has no room (600px, or half the window if that is less). */
export function sidebarBounds(width: number, windowWidth: number): number {
  var most = Math.max(SIDEBAR_MIN, Math.min(600, Math.floor(windowWidth / 2)));
  return Math.round(Math.min(most, Math.max(SIDEBAR_MIN, width)));
}

/**
 * The pane's width: `keepIt` once it is where it was taken to (at the end of
 * a drag), not at every step of the way there.
 */
export function setSidebarWidth(width: number, keepIt: boolean): void {
  var w = sidebarBounds(width, window.innerWidth);
  sidebarWidth.set(w);
  if (keepIt) remember('diffnote-sidebar-w', String(w), { sidebar_width: w });
}

/**
 * Whether the lists beside the diff are out of the way just now. Not kept:
 * it is for making room for a moment, so a page opened again has them back.
 */
export const sidebarHidden = atom(false);

export function setLayout(next: Layout): void {
  remember('diffnote-layout', next, { layout: next });
  chosenLayout.set(next);
}

export function setHideResolved(on: boolean): void {
  remember('diffnote-hide-resolved', on ? '1' : '0', { hide_resolved: on });
  hideResolved.set(on);
}

export function setWrapLines(on: boolean): void {
  remember('diffnote-wrap', on ? '1' : '0', { wrap: on });
  wrapLines.set(on);
}

export function setSyncScroll(on: boolean): void {
  remember('diffnote-sync-scroll', on ? '1' : '0', { sync_scroll: on });
  syncScroll.set(on);
}

export function setIgnoreWhitespace(on: boolean): void {
  ignoreWhitespace.set(on);
}

export function setSidebarHidden(on: boolean): void {
  sidebarHidden.set(on);
}

/**
 * What the page starts with: what was kept from last time (the user
 * settings' first, `saved`, then the browser's), and, where nothing was, the
 * review's own setting and the width of the window.
 *
 * Only when the page opens: a window made wider afterwards doesn't rearrange
 * the diff under the reader.
 */
export function startView(reviewIgnoresWhitespace: boolean, saved?: ViewPrefs): void {
  const pref = saved || {};
  const flag = function (value: boolean | undefined, key: string, fallback: string): boolean {
    return value !== undefined ? value : kept(key, fallback) !== '0';
  };
  const stored = pref.layout || kept('diffnote-layout', '');
  wide.set(window.matchMedia('(min-width: 900px)').matches);
  chosenLayout.set(
    stored === 'split' || stored === 'unified'
      ? stored
      : window.matchMedia('(min-width: 1200px)').matches
        ? 'split'
        : 'unified',
  );
  hideResolved.set(flag(pref.hide_resolved, 'diffnote-hide-resolved', '1'));
  wrapLines.set(flag(pref.wrap, 'diffnote-wrap', '1'));
  syncScroll.set(flag(pref.sync_scroll, 'diffnote-sync-scroll', '1'));
  const width = pref.sidebar_width || Number(kept('diffnote-sidebar-w', '')) || SIDEBAR_DEFAULT;
  sidebarWidth.set(sidebarBounds(width, window.innerWidth));
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
