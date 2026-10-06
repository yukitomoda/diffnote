// The first screen's choices, apart from how they are drawn (Setup.tsx), so
// that what a repository is reviewed from and what is sent can be tested
// without a page. (The 「リポジトリ」 screen adds a repository with the same
// rows: `MANUAL`, `baseOf`, `targetSettled`.)
import { lib } from '../lib.ts';
import type { ReviewKind, SetupCandidate, SetupChoice, SetupData, SetupPreview, SetupRepo, SetupSpan } from '../model.ts';

/** The value of the base choice that stands for a commit typed in by hand. */
export const MANUAL = 'manual';

/** One repository as the screen holds it: which base is chosen (a
 * candidate's `rev`, or `MANUAL`), what was typed, and what the server
 * said that would review. */
export interface RepoRow {
  info: SetupRepo;
  /** Part of the review (a review of several lets one be left out). */
  on: boolean;
  base: string;
  manual: string;
  /** What the typed commit would review, once the server has said. */
  preview: SetupPreview | null;
  error: string;
  checking: boolean;
  /** What is compared up to, each time: `HEAD`, or a branch or commit
   * typed in, and what the server said of that. */
  target: string;
  targetPreview: SetupSpan | null;
  targetError: string;
  targetChecking: boolean;
  /** The first screen: whether its base was chosen for it alone (else the
   * one chosen for all, `Bulk`), and what its range takes in, as the server
   * said. */
  custom: boolean;
  span: SetupSpan | null;
  spanError: string;
  spanChecking: boolean;
}

/** How every repository is reviewed from, unless one is chosen for it: from
 * its suggested base (where its work left the default branch, or the commit
 * before `HEAD`), or from the last `count` commits. */
export interface Bulk {
  mode: 'suggested' | 'last';
  count: number;
}

/** Whether a target typed in is settled (`HEAD` needs no asking). */
export function targetSettled(row: RepoRow): boolean {
  var t = row.target.trim();
  return t === 'HEAD' || (!!t && !row.targetChecking && !!row.targetPreview);
}

/** What the screen holds, from what it was given to what is chosen. */
export interface SetupState {
  kind: ReviewKind;
  title: string;
  snapshot: 'changed' | 'full';
  git: RepoRow | null;
  repos: RepoRow[];
  bulk: Bulk;
  /** The repository whose graph is shown (its path; empty: the one). */
  active: string;
}

export function rowOf(info: SetupRepo): RepoRow {
  return {
    info: info,
    on: true,
    base: info.candidates.length ? info.candidates[0].rev : MANUAL,
    manual: '',
    preview: null,
    error: '',
    checking: false,
    target: 'HEAD',
    targetPreview: null,
    targetError: '',
    targetChecking: false,
    custom: false,
    span: null,
    spanError: '',
    spanChecking: false,
  };
}

/** Where the screen starts: what the directory looks like, with the best
 * base of each repository chosen. Of several, none is in until it is
 * ticked (a directory can hold many more than a review is about). */
export function stateOf(setup: SetupData): SetupState {
  return {
    kind: setup.kind,
    title: setup.title || '',
    snapshot: setup.snapshot || 'changed',
    git: setup.git ? rowOf(setup.git) : null,
    repos: setup.repos.map(function (info) { return Object.assign(rowOf(info), { on: false }); }),
    bulk: { mode: 'suggested', count: 3 },
    active: setup.kind === 'workspace' && setup.repos.length ? setup.repos[0].path : '',
  };
}

/** What a row compares up to: what is typed, or `HEAD`. */
export function targetOf(row: RepoRow): string {
  return row.target.trim() || 'HEAD';
}

/** What a row is reviewed from: what was chosen for it alone, or else what
 * is chosen for all. */
export function baseFor(state: SetupState, row: RepoRow): string {
  if (row.custom) return baseOf(row);
  if (state.bulk.mode === 'last') return targetOf(row) + '~' + Math.max(1, Math.floor(state.bulk.count) || 1);
  return row.info.suggested.id;
}

/** How many repositories, commits and files the review takes in, as far as
 * the server has said. */
export function totalsOf(state: SetupState): { repos: number; commits: number; files: number } {
  var rows = rowsOf(state);
  return rows.reduce(function (t, r) {
    return { repos: t.repos, commits: t.commits + (r.span ? r.span.commits : 0), files: t.files + (r.span ? r.span.files : 0) };
  }, { repos: rows.length, commits: 0, files: 0 });
}

/** What a candidate is called: each of its names, joined. */
export function candidateLabel(c: SetupCandidate): string {
  return c.names.map(function (n) {
    if (n.kind === 'fork') return lib.mf('ui.setup.name_fork', { branch: n.branch || '' });
    if (n.kind === 'branch') return lib.mf('ui.setup.name_branch', { branch: n.branch || '' });
    return lib.m('ui.setup.name_head');
  }).join(lib.m('ui.setup.name_join'));
}

/** How many files a commit's tree holds, said in a line. */
export function previewText(files: number): string {
  return lib.mf('ui.setup.preview', { files: String(files) });
}

/** How many commits a comparison takes in, said in a line. */
export function spanText(commits: number): string {
  if (commits === 0) return lib.m('ui.setup.span_none');
  return lib.mf('ui.setup.span', { commits: String(commits) });
}

/** What a range takes in, said in a line: how many commits and files. */
export function rangeText(span: SetupSpan): string {
  if (span.commits === 0 && span.files === 0) return lib.m('ui.setup.span_none');
  return lib.mf('ui.setup.range', { commits: String(span.commits), files: String(span.files) });
}

/** The commit a row's base names: the candidate's, or what was typed. */
export function baseOf(row: RepoRow): string {
  return row.base === MANUAL ? row.manual.trim() : row.base;
}

/** The rows a choice is made of: the one repository, or the ones kept. */
function rowsOf(state: SetupState): RepoRow[] {
  if (state.kind === 'git') return state.git ? [state.git] : [];
  if (state.kind === 'workspace') return state.repos.filter(function (r) { return r.on; });
  return [];
}

/** Why the choice can't be sent yet (a message key), or `null`: every
 * repository's range said by the server to be one, and something in it. */
export function problemOf(state: SetupState): string | null {
  var rows = rowsOf(state);
  if (state.kind === 'git' && !state.git) return 'ui.setup.not_a_repo';
  if (state.kind === 'workspace' && rows.length === 0) return 'ui.setup.no_repos_chosen';
  for (var i = 0; i < rows.length; i++) {
    var row = rows[i];
    if (!baseFor(state, row)) return 'ui.setup.base_needed';
    if (row.spanError) return 'ui.setup.range_invalid';
    if (row.spanChecking || !row.span) return 'ui.setup.range_unchecked';
  }
  if (rows.length && totalsOf(state).files === 0) return 'ui.setup.nothing_to_review';
  return null;
}

/** What is sent to make the review (see `Choice` in `src/setup.rs`). */
export function choiceOf(state: SetupState): SetupChoice {
  var choice: SetupChoice = { kind: state.kind };
  var title = state.title.trim();
  if (title) choice.title = title;
  if (state.kind === 'git') {
    choice.snapshot = state.snapshot;
    choice.base = state.git ? baseFor(state, state.git) : '';
  } else if (state.kind === 'workspace') {
    choice.snapshot = state.snapshot;
    choice.repos = rowsOf(state).map(function (r) {
      var one: { path: string; base: string; target?: string } = { path: r.info.path, base: baseFor(state, r) };
      if (targetOf(r) !== 'HEAD') one.target = targetOf(r);
      return one;
    });
  }
  return choice;
}

/** A repository named by hand, added to the list (once). */
export function withRepo(rows: RepoRow[], info: SetupRepo): RepoRow[] | null {
  if (rows.some(function (r) { return r.info.path === info.path; })) return null;
  return rows.concat([rowOf(info)]);
}
