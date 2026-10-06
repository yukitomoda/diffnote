// What the left pane shows (the files and threads, or the search), what is
// searched for, and which of what was found is the one gone to. Stores, as
// they are written from the keys (Ctrl+F) as well as from the pane.
import { atom } from 'nanostores';
import { setSidebarHidden } from './view.ts';

export type SidebarView = 'files' | 'search';

export const sidebarView = atom<SidebarView>('files');

export const searchText = atom('');
export const searchCase = atom(false);
/** Which hit is the one gone to (its place in `SearchResult.all`); -1 for
 * none yet. */
export const searchCurrent = atom(-1);
/** Asks the search's box to take the keys (each time it changes). */
export const searchFocus = atom(0);

/** The search, in the left pane (shown, if it was hidden), its box taking the keys. */
export function openSearch(): void {
  setSidebarHidden(false);
  sidebarView.set('search');
  searchFocus.set(searchFocus.get() + 1);
}

/** Back to the files and threads (what was searched for is kept). */
export function showFiles(): void {
  sidebarView.set('files');
}
