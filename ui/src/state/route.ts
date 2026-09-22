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
export const SECTIONS = ['general', 'settings', 'attachments', 'user'] as const;
export type Section = (typeof SECTIONS)[number];

/** The revision shown, as the model has them (0-based; the address is 1-based). */
export const current = atom(0);
export const screen = atom<Section | null>(null);
/** An earlier revision this one is shown against, or `null` for the base. */
export const against = atom<number | null>(null);
/** The last place jumped to: in the address, so a jump can be retraced. */
export const at = atom<At | null>(null);

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
  return (
    '#' +
    lib.formatHash({
      rev: current.get() + 1,
      screen: screen.get(),
      against: against.get(),
      at: at.get(),
    })
  );
}

/** Where the page is, from an address (the revision is 1-based there). */
export function showRoute(route: PageState): void {
  current.set(route.rev - 1);
  screen.set(isSection(route.screen) ? route.screen : null);
  against.set(route.against);
  at.set(route.at);
}

export function showRevision(index: number): void {
  current.set(index);
}

export function showScreen(section: Section | null): void {
  screen.set(section);
}

/** Show this revision against an earlier one, or against the base (`null`). */
export function compareWith(revision: number | null): void {
  against.set(revision);
}

export function jumpedTo(place: At | null): void {
  at.set(place);
}

/**
 * Where the page starts: what the address says, where it says anything this
 * review has, and the last revision otherwise.
 */
export function startRoute(hash: string, revisions: number): PageState | null {
  const route = routeOfHash(hash, revisions);
  if (route) showRoute(route);
  else current.set(revisions - 1);
  return route;
}

// Whether the address has been written once already. The first write fills in
// where the page already is; every one after it is somewhere the reader went,
// and so is something back should return from.
let everWritten = false;

/**
 * Write the address for where the page is now, unless it already says that.
 * `fromAddress` is a move that came from the address itself (the back button):
 * there is nothing to write, only to remember that there is now something to
 * go back to.
 */
export function writeAddress(fromAddress: boolean): void {
  if (fromAddress) {
    everWritten = true;
    return;
  }
  const hash = hashOfRoute();
  if (hash !== location.hash) {
    if (everWritten) history.pushState(null, '', hash);
    else history.replaceState(null, '', hash);
  }
  everWritten = true;
}

/** For a test: as if the page had just opened. */
export function forgetAddress(): void {
  everWritten = false;
}
