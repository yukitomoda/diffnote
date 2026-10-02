// The files marked as looked at, in each revision.
//
// A file is marked in a revision, with what the file was at the time (its two
// versions' digests), and taking a mark back is said too (`null`). What a
// file is in a revision is its nearest mark, in that revision or an earlier
// one: a file that is the same as when it was marked stays looked at in the
// revisions after, and one that has become another one since is not. On the
// served page the marks are the signed-in name's, kept in the review: the
// model brings them with each revision, and a mark is sent to the server as it
// is made. An exported page keeps them only while it is open -- it is one
// reader going through one page.
import { atom } from 'nanostores';
import type { Answer, FileData, ViewModel } from '../model.ts';

export interface Marks {
  /** The revisions' ids, oldest first: how far back a mark reaches. */
  order: string[];
  /** By revision id, then by path: what the file was when it was marked, or
   * `null` where the mark was taken back. */
  by: Record<string, Record<string, string | null>>;
}

export const seen = atom<Marks>({ order: [], by: {} });

/** Where a mark is kept (the served page), or `null` (only here). */
type Keep = (revision: string, path: string, sig: string, viewed: boolean) => Promise<Answer<unknown>>;
let keep: Keep | null = null;

export function keepWith(f: Keep | null): void {
  keep = f;
}

/** The marks a model brings (on the served page, of the name it is served to;
 * an exported page brings none). */
export function marksOf(model: ViewModel): Marks {
  const by: Marks['by'] = {};
  model.revisions.forEach(function (r) {
    if (r.viewed && Object.keys(r.viewed).length) by[r.id] = Object.assign({}, r.viewed);
  });
  return { order: model.revisions.map(function (r) { return r.id; }), by: by };
}

function mark(file: FileData): string {
  return file.sig || '';
}

const has = (o: object, key: string) => Object.prototype.hasOwnProperty.call(o, key);

/** The revision's own mark of `path` (`undefined`: none). */
function own(marks: Marks, revision: string, path: string): string | null | undefined {
  const here = marks.by[revision];
  return here && has(here, path) ? here[path] : undefined;
}

/** The nearest mark of `path` in a revision before `revision`. */
function earlier(marks: Marks, revision: string, path: string): string | null | undefined {
  for (let i = marks.order.indexOf(revision) - 1; i >= 0; i--) {
    const found = own(marks, marks.order[i], path);
    if (found !== undefined) return found;
  }
  return undefined;
}

/** The mark of `path` that holds in `revision`: its own, or the nearest
 * earlier one's. */
function nearest(marks: Marks, revision: string, path: string): string | null | undefined {
  const mine = own(marks, revision, path);
  return mine !== undefined ? mine : earlier(marks, revision, path);
}

export function isViewed(file: FileData, marks: Marks, revision: string): boolean {
  return nearest(marks, revision, file.path) === mark(file);
}

/** Sets this revision's own mark of `path` (`undefined`: none of its own). */
function set(revision: string, path: string, value: string | null | undefined): void {
  const marks = seen.get();
  const here = Object.assign({}, marks.by[revision] || {});
  if (value === undefined) delete here[path];
  else here[path] = value;
  const by = Object.assign({}, marks.by);
  if (Object.keys(here).length) by[revision] = here;
  else delete by[revision];
  seen.set({ order: marks.order, by: by });
}

export function toggleViewed(file: FileData, revision: string): void {
  const marks = seen.get();
  const was = own(marks, revision, file.path);
  const now = !isViewed(file, marks, revision);
  // Taken back: said so only where an earlier mark would otherwise reach
  // (the server decides the same for what it keeps).
  const made = now ? mark(file) : earlier(marks, revision, file.path) !== undefined ? null : undefined;
  set(revision, file.path, made);
  if (!keep) return;
  keep(revision, file.path, mark(file), now).then(function (res) {
    // Not kept: the page says what the review has, as long as nothing has
    // been done to the file since.
    if (!res.ok && own(seen.get(), revision, file.path) === made) set(revision, file.path, was);
  });
}
