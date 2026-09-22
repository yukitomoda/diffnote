// The files marked as looked at.
//
// A file is marked by path, with what the file was at the time (its two
// versions' digests): a file that has become another one since is not marked
// any more, so a revision that changes it brings it back. Nothing of this is
// recorded in the review -- it is one reader going through one page.
import { atom } from 'nanostores';
import type { FileData } from '../model.ts';

/** What each marked file was when it was marked, by path. */
export const seen = atom<Record<string, string>>({});

function mark(file: FileData): string {
  return file.sig || '';
}

export function isViewed(file: FileData, marks: Record<string, string>): boolean {
  return Object.prototype.hasOwnProperty.call(marks, file.path) && marks[file.path] === mark(file);
}

export function toggleViewed(file: FileData): void {
  const marks = seen.get();
  const next = Object.assign({}, marks);
  if (isViewed(file, marks)) delete next[file.path];
  else next[file.path] = mark(file);
  seen.set(next);
}
