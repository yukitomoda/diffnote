// Searching a revision for some text: what it finds, apart from how it is
// shown (nav/SearchPane.tsx) or reached, so that it can be tried without a
// page.
//
// What is found is one kind of thing wherever it was found: a file's name, a
// line of a file (on its old or new side), or a comment. The lines are
// those of the diff today -- the page has them all, folded files too; the
// lines the diff leaves out, and the files the review doesn't have, are the
// server's to search, and come back as the same `LineHit`s.
import { lib } from './lib.ts';
import type { FileData, Placement, Side, ThreadData, Token } from './model.ts';

export interface SearchQuery {
  text: string;
  /** Whether `A` and `a` are told apart. */
  caseSensitive: boolean;
}

/** A line of a file the diff leaves out, with its text: what the server
 * answers with, or what an exported page carries. */
export interface GapLine {
  path: string;
  /** Its number on the new side (it is the same on both). */
  line: number;
  text: string;
}

/** Where in a text it was found: `[start, end)`, in UTF-16 units. */
export type Span = [number, number];

/** A line it was found in: of which file, on which side and which line, and
 * where in it (the `nth` time it is in that line). */
export interface LineHit {
  kind: 'line';
  path: string;
  side: Side;
  line: number;
  text: string;
  span: Span;
  nth: number;
  /** A line the diff leaves out: to be shown before it is gone to. */
  gap?: boolean;
}

/** A file whose name (its path) it is in. */
export interface NameHit {
  kind: 'name';
  path: string;
  span: Span;
}

/** A comment it was found in: of which thread, the comment's place in it,
 * and where in its text (said once, as text). */
export interface CommentHit {
  kind: 'comment';
  thread: string;
  comment: string;
  author: string;
  text: string;
  span: Span;
  nth: number;
}

export type Hit = LineHit | NameHit | CommentHit;

/** What was found in one file: its name, if that is a match, and its lines. */
export interface FileGroup {
  path: string;
  name: NameHit | null;
  lines: LineHit[];
}

export interface SearchResult {
  files: FileGroup[];
  comments: CommentHit[];
  /** Every hit, in the order the next one is gone to: file by file (its
   * name, then its lines), then the comments. */
  all: Hit[];
  /** Whether there were more, and the rest left out (past the most it
   * finds, or the time it may take). */
  capped: boolean;
  /** Whether it was the time that ran out. */
  timedOut: boolean;
}

/** The most it finds, unless asked for all: past that, the text is too
 * common to be worth listing. */
export const SEARCH_MOST = 1000;

/** How long a search for all may take (ms): past that, what it has found is
 * what there is (so that a text in nearly every line can't hold the page).
 * The server stops after as long. */
export const SEARCH_ALL_TIME = 30000;

export interface SearchLimits {
  /** The most it finds (`SEARCH_MOST` if not said; `Infinity` for all). */
  most?: number;
  /** When it stops, as `Date.now()` says (never, if not said). */
  until?: number;
}

/** Where `query` is in `text`, each time (not one inside another). */
export function occurrences(text: string, query: SearchQuery): Span[] {
  var out: Span[] = [];
  if (!query.text) return out;
  var hay = query.caseSensitive ? text : text.toLowerCase();
  var needle = query.caseSensitive ? query.text : query.text.toLowerCase();
  // (Lower-casing keeps the length of what is searched for the scripts it
  // is used on; where it wouldn't, nothing is found rather than a wrong place.)
  if (hay.length !== text.length) return out;
  for (var at = hay.indexOf(needle); at >= 0; at = hay.indexOf(needle, at + needle.length)) {
    out.push([at, at + needle.length]);
  }
  return out;
}

/** A line's text, as its pieces are. */
export function lineText(tokens: Token[]): string {
  return tokens.map(function (t) { return typeof t === 'string' ? t : t[1]; }).join('');
}

/** The lines the diffs leave out that an exported page carries (in pieces,
 * or as text). */
export function carriedGapLines(files: FileData[]): GapLine[] {
  var out: GapLine[] = [];
  files.forEach(function (f) {
    (f.gaps || []).forEach(function (g) {
      if (!g) return;
      if (g.t) g.t.forEach(function (pieces, i) { out.push({ path: f.path, line: g.w + i, text: lineText(pieces) }); });
      else if (g.s) g.s.forEach(function (text, i) { out.push({ path: f.path, line: g.w + i, text: text }); });
    });
  });
  return out;
}

/**
 * What `query` finds in a revision: its files' names and lines (`files`, in
 * the order the page shows them), and the comments of `threads`. A line both
 * sides have is counted once, on the new side; a removed one on the old.
 * `gaps` are lines the diffs leave out, to search as well: each goes where
 * the page has it, between the diff's lines (one the diff has after all is
 * not counted twice).
 */
export function searchRevision(files: FileData[], threads: ThreadData[], query: SearchQuery, gaps?: GapLine[], limits?: SearchLimits): SearchResult {
  var result: SearchResult = { files: [], comments: [], all: [], capped: false, timedOut: false };
  if (!query.text) return result;
  var most = limits && limits.most != null ? limits.most : SEARCH_MOST;
  var until = limits && limits.until;
  var byPath: Record<string, GapLine[]> = {};
  (gaps || []).forEach(function (g) { (byPath[g.path] = byPath[g.path] || []).push(g); });
  // (The time is looked at now and then: every so many lines.)
  var looked = 0;
  var full = function (): boolean {
    if (result.capped) return true;
    if (until != null && ++looked % 256 === 0 && Date.now() > until) {
      result.capped = result.timedOut = true;
      return true;
    }
    if (result.all.length < most) return false;
    result.capped = true;
    return true;
  };
  files.forEach(function (f) {
    if (full()) return;
    var group: FileGroup = { path: f.path, name: null, lines: [] };
    var inName = occurrences(f.path, query)[0];
    if (inName) {
      group.name = { kind: 'name', path: f.path, span: inName };
      result.all.push(group.name);
    }
    // Each line with where the page has it: by its new number (a removed one
    // just after the new line before it).
    var found: { at: number; hit: LineHit }[] = [];
    var inDiff: Record<number, boolean> = {};
    var lastNew = 0;
    (f.hunks || []).forEach(function (h) {
      h.rows.forEach(function (row) {
        if (row.n != null) { inDiff[row.n] = true; lastNew = row.n; }
        if (full()) return;
        var side: Side = row.k === 'd' ? 'old' : 'new';
        var line = side === 'old' ? row.o : row.n;
        if (line == null) return;
        var text = lineText(row.t);
        var at = side === 'old' ? lastNew + 0.5 : line;
        occurrences(text, query).forEach(function (span, nth) {
          found.push({ at: at, hit: { kind: 'line', path: f.path, side: side, line: line!, text: text, span: span, nth: nth } });
        });
      });
    });
    (byPath[f.path] || []).forEach(function (g) {
      if (inDiff[g.line] || full()) return;
      occurrences(g.text, query).forEach(function (span, nth) {
        found.push({ at: g.line, hit: { kind: 'line', path: f.path, side: 'new', line: g.line, text: g.text, span: span, nth: nth, gap: true } });
      });
    });
    found.sort(function (a, b) { return a.at - b.at; });
    found.forEach(function (x) {
      if (full()) return;
      group.lines.push(x.hit);
      result.all.push(x.hit);
    });
    if (group.name || group.lines.length) result.files.push(group);
  });
  threads.forEach(function (t) {
    t.comments.forEach(function (c) {
      if (c.deleted || full()) return;
      var text = lib.plainText(c.doc);
      occurrences(text, query).forEach(function (span, nth) {
        if (full()) return;
        var hit: CommentHit = { kind: 'comment', thread: t.id, comment: c.id, author: c.author, text: text, span: span, nth: nth };
        result.comments.push(hit);
        result.all.push(hit);
      });
    });
  });
  return result;
}

/** A piece of a line around what was found, short enough for a row of the
 * list: up to `before` characters ahead of it, the rest after. */
export function excerpt(text: string, span: Span, before: number, most: number): { head: string; found: string; tail: string } {
  var from = Math.max(0, span[0] - before);
  // (Leading white space says nothing in a list.)
  var head = text.slice(from, span[0]);
  if (from === 0) head = head.replace(/^\s+/, '');
  else head = '…' + head;
  var tail = text.slice(span[1], span[1] + Math.max(0, most - (span[0] - from) - (span[1] - span[0])));
  if (span[1] + tail.length < text.length) tail += '…';
  return { head: head, found: text.slice(span[0], span[1]), tail: tail };
}

/** Where a thread is, said as the thread list says it. */
export function placeOf(placement: Placement | undefined): string {
  return placement ? lib.shortLocation(placement) : '';
}
