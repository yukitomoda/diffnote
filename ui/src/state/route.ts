// Where the page is: which revision is shown, which settings screen (if any),
// what the revision is being compared against, and the last place jumped to.
//
// This is the part of the page's state that belongs in the address, so that the
// browser's back and forward buttons retrace it. It is kept here, away from the
// components, because it is written from all over the page (a tab, a link in a
// comment, the thread list, the settings screens) and because what it does --
// what the address should say, and what an address means -- can then be tried
// without a browser.
import { atom } from 'nanostores';
import { lib } from '../lib.ts';
import type { At, PageState } from '../lib.ts';

/** The settings screens, in the order the nav lists them. */
export const SECTIONS = ['timeline', 'general', 'settings', 'attachments', 'user'] as const;
export type Section = (typeof SECTIONS)[number];

/**
 * The same screens in the order and the grouping a reader is given them in:
 * the review itself and what it holds -- 全般 first, being where the screen
 * opens -- then what is set. Here rather than with the screen
 * that draws them, so that a screen an address can carry and a screen anyone
 * can reach cannot drift apart (`ui/src/test/route.test.js` checks it).
 */
export const SECTION_GROUPS: Section[][] = [
  ['general', 'timeline', 'attachments'],
  ['settings', 'user'],
];

/** Where the page is. One value, not four, so that a move is one change: the
 * address is written from it, and writing it twice for one move would leave
 * the back button with somewhere to go that the reader was never at. */
export interface Route {
  /** The revision shown, as the model has them (0-based; the address is 1-based). */
  rev: number;
  screen: Section | null;
  /** An earlier revision this one is shown against, or `null` for the base. */
  against: number | null;
  /** The last place jumped to: in the address, so a jump can be retraced. */
  at: At | null;
}

export const route = atom<Route>({ rev: 0, screen: null, against: null, at: null });

function change(part: Partial<Route>): void {
  route.set(Object.assign({}, route.get(), part));
}

export function isSection(name: string | null | undefined): name is Section {
  return !!name && (SECTIONS as readonly string[]).indexOf(name) >= 0;
}

/**
 * What an address means for a review of `revisions` revisions, or `null` if it
 * says nothing this page can use (no revision, or one it doesn't have).
 */
export function routeOfHash(hash: string, revisions: number): PageState | null {
  const parsed = lib.parseHash(hash);
  if (!parsed || parsed.rev < 1 || parsed.rev > revisions) return null;
  return {
    rev: parsed.rev,
    screen: isSection(parsed.screen) ? parsed.screen : null,
    against: parsed.against,
    at: parsed.at,
  };
}

/** The address for where the page is now (with its `#`). */
export function hashOfRoute(): string {
  const here = route.get();
  return (
    '#' +
    lib.formatHash({
      rev: here.rev + 1,
      screen: here.screen,
      against: here.against,
      at: here.at,
    })
  );
}

/**
 * Where the page is, from an address (the revision is 1-based there): the back
 * button, or the page opening. There is nothing to write back.
 */
export function showRoute(state: PageState): void {
  fromAddress = true;
  route.set({
    rev: state.rev - 1,
    screen: isSection(state.screen) ? state.screen : null,
    against: state.against,
    at: state.at,
  });
  fromAddress = false;
}

// Each of these is one move: one change to the route, and so one thing for
// the back button to undo. (Two changes would be two entries in the history,
// and the reader would have to press back twice for something they did once.)

/** Show this revision, leaving whatever screen or place the reader was at. */
export function showRevision(index: number): void {
  change({ rev: index });
}

/** A tab: the revision, from the top, with no screen over it. */
export function openRevision(index: number): void {
  change({ rev: index, screen: null, at: null });
}

/** A jump (a link in a comment, the thread list, the 添付 screen). */
export function goTo(index: number, place: At): void {
  change({ rev: index, screen: null, at: place });
}

export function showScreen(section: Section | null): void {
  change({ screen: section });
}

/** Show this revision against an earlier one, or against the base (`null`). */
export function compareWith(revision: number | null): void {
  change({ against: revision });
}

/**
 * Where the page starts: what the address says, where it says anything this
 * review has, and the last revision otherwise. The address is then written
 * once, to say the same thing in full, without adding to the history.
 */
export function startRoute(hash: string, revisions: number): PageState | null {
  const found = routeOfHash(hash, revisions);
  if (found) showRoute(found);
  else {
    fromAddress = true;
    route.set({ rev: revisions - 1, screen: null, against: null, at: null });
    fromAddress = false;
  }
  writeAddress();
  return found;
}

// Whether the address has been written once already. The first write fills in
// where the page already is; every one after it is somewhere the reader went,
// and so is something back should return from.
let everWritten = false;
// Whether the move being made came from the address itself (the back button),
// where there is nothing to write back.
let fromAddress = false;

/** Write the address for where the page is now, unless it already says that. */
function writeAddress(): void {
  const hash = hashOfRoute();
  if (hash !== location.hash) {
    if (everWritten) history.pushState(null, '', hash);
    else history.replaceState(null, '', hash);
  }
  everWritten = true;
}

/**
 * Keep the address in step with where the page is, for as long as the page is
 * open. This listens to the route itself rather than following what is drawn:
 * every move has to reach the address as it is made, since each one is a thing
 * the back button undoes, and a page that draws two moves at once (which it
 * may: what is drawn is caught up with in its own time) would lose one.
 */
export function followRoute(): () => void {
  return route.listen(function () {
    if (fromAddress) {
      everWritten = true;
      return;
    }
    writeAddress();
  });
}

/** For a test: as if the page had just opened. */
export function forgetAddress(): void {
  everWritten = false;
}
