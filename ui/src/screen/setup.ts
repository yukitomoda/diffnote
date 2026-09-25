// The first screen's choices, apart from how they are drawn (Setup.tsx), so
// that what a repository is offered and what is sent can be tested without
// a page.
import { lib } from '../lib.ts';
import type { ReviewKind, SetupCandidate, SetupChoice, SetupData, SetupPreview, SetupRepo } from '../model.ts';

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
}

/** What the screen holds, from what it was given to what is chosen. */
export interface SetupState {
  kind: ReviewKind;
  title: string;
  snapshot: 'changed' | 'full';
  git: RepoRow | null;
  repos: RepoRow[];
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
  };
}

/** What a candidate is called: each of its names, joined. */
export function candidateLabel(c: SetupCandidate): string {
  return c.names.map(function (n) {
    if (n.kind === 'fork') return lib.mf('ui.setup.name_fork', { branch: n.branch || '' });
    if (n.kind === 'branch') return lib.mf('ui.setup.name_branch', { branch: n.branch || '' });
    return lib.m('ui.setup.name_head');
  }).join(lib.m('ui.setup.name_join'));
}

/** What choosing a base would review, said in a line. */
export function previewText(commits: number, files: number): string {
  if (commits === 0 && files === 0) return lib.m('ui.setup.preview_none');
  return lib.mf('ui.setup.preview', { commits: String(commits), files: String(files) });
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

/** Why the choice can't be sent yet (a message key), or `null`. */
export function problemOf(state: SetupState): string | null {
  var rows = rowsOf(state);
  if (state.kind === 'git' && !state.git) return 'ui.setup.not_a_repo';
  if (state.kind === 'workspace' && rows.length === 0) return 'ui.setup.no_repos_chosen';
  for (var i = 0; i < rows.length; i++) {
    var row = rows[i];
    if (!baseOf(row)) return 'ui.setup.base_needed';
    if (row.base === MANUAL && (row.checking || !row.preview)) return 'ui.setup.base_unchecked';
  }
  return null;
}

/** What is sent to make the review (see `Choice` in `src/setup.rs`). */
export function choiceOf(state: SetupState): SetupChoice {
  var choice: SetupChoice = { kind: state.kind };
  var title = state.title.trim();
  if (title) choice.title = title;
  if (state.kind === 'git') {
    choice.snapshot = state.snapshot;
    choice.base = state.git ? baseOf(state.git) : '';
  } else if (state.kind === 'workspace') {
    choice.snapshot = state.snapshot;
    choice.repos = rowsOf(state).map(function (r) { return { path: r.info.path, base: baseOf(r) }; });
  }
  return choice;
}

/** A repository named by hand, added to the list (once). */
export function withRepo(rows: RepoRow[], info: SetupRepo): RepoRow[] | null {
  if (rows.some(function (r) { return r.info.path === info.path; })) return null;
  return rows.concat([rowOf(info)]);
}
