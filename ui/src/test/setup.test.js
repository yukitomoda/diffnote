import test from 'node:test';
import assert from 'node:assert/strict';
import { lib } from '../lib.ts';
import { MANUAL, baseFor, baseOf, candidateLabel, choiceOf, previewText, problemOf, rangeText, rowOf, spanText, stateOf, targetSettled, totalsOf, withRepo } from '../screen/setup.ts';

lib.setMessages({
  'ui.setup.name_fork': '{branch} からの分岐点',
  'ui.setup.name_branch': '{branch} の先端',
  'ui.setup.name_head': 'HEAD',
  'ui.setup.name_join': ' / ',
  'ui.setup.preview': '{files} ファイル',
  'ui.setup.span': '{commits} コミット',
  'ui.setup.span_none': '差分なし',
  'ui.setup.range': '{commits} コミット・{files} ファイル',
});

const commit = (id, subject) => ({ id, short: id.slice(0, 7), subject });
const candidate = (rev, names, files) => Object.assign(commit(rev.padEnd(40, '0'), 's-' + rev), { rev, names, files });
const suggested = Object.assign(commit('f0'.padEnd(40, '0'), 'fork'), { why: 'fork' });
const repo = (path, candidates) => ({ path, branch: 'feature', head: commit('head'.padEnd(40, 'f'), 'c9'), default_branch: 'main', candidates, suggested });
const span = (commits, files) => Object.assign(commit('t'.padEnd(40, '1'), 'tip'), { commits, files, ids: [], from: suggested.id });
/** A row as it is once the server has said what its range takes in. */
const said = (row, commits = 1, files = 2) => Object.assign({}, row, { span: span(commits, files), spanChecking: false, spanError: '' });
const fork = candidate('abc1234', [{ kind: 'fork', branch: 'main' }], 5);
const tip = candidate('main', [{ kind: 'branch', branch: 'main' }, { kind: 'head' }], 7);
const setup = (kind, git, repos) => ({
  review: 'r.diffnote', kind, kinds: { git: { ok: !!git }, workspace: { ok: repos.length > 0 }, raw: { ok: true } },
  git, repos, depth: 4,
});

test('a candidate is named by every name it goes by, and what it would review is said', () => {
  assert.equal(candidateLabel(fork), 'main からの分岐点');
  assert.equal(candidateLabel(tip), 'main の先端 / HEAD');
  assert.equal(previewText(5), '5 ファイル');
  assert.equal(spanText(3), '3 コミット');
  assert.equal(spanText(0), '差分なし');
});

test('the screen starts from what the directory looks like', () => {
  const one = stateOf(setup('git', repo('', [fork, tip]), []));
  assert.equal(one.kind, 'git');
  assert.equal(one.snapshot, 'changed');
  assert.equal(one.title, '');
  assert.deepEqual(one.bulk, { mode: 'suggested', count: 3 });
  assert.equal(one.active, '', 'the one repository');
  const several = stateOf(Object.assign(setup('workspace', null, [repo('a', [tip]), repo('b', [fork, tip])]), { title: 'T', snapshot: 'full' }));
  assert.equal(several.git, null);
  assert.deepEqual(several.repos.map((r) => [r.info.path, r.on, r.custom]), [['a', false, false], ['b', false, false]], 'none is in until ticked');
  assert.equal(several.active, 'a', 'the first is the one whose history is shown');
  assert.equal(several.snapshot, 'full');
  assert.equal(several.title, 'T');
  assert.equal(rowOf(repo('c', [])).base, MANUAL, 'nothing offered: typed in (the 「リポジトリ」 screen)');
});

test('a repository is reviewed from what is chosen for all, unless one is chosen for it alone', () => {
  const ws = stateOf(setup('workspace', null, [repo('a', [tip])]));
  const a = ws.repos[0];
  assert.equal(baseFor(ws, a), suggested.id, 'its suggested base');
  ws.bulk = { mode: 'last', count: 3 };
  assert.equal(baseFor(ws, a), 'HEAD~3', 'the last commits, before what it goes up to');
  assert.equal(baseFor(ws, Object.assign({}, a, { target: ' release ' })), 'release~3');
  assert.equal(baseFor(Object.assign({}, ws, { bulk: { mode: 'last', count: 0 } }), a), 'HEAD~1', 'one at least');
  const own = Object.assign({}, a, { custom: true, base: MANUAL, manual: ' v1.2 ' });
  assert.equal(baseFor(ws, own), 'v1.2', 'chosen for it alone');
  assert.equal(baseOf(own), 'v1.2');
});

test('what is sent is the kind, the base of each repository kept, and the rest only where it applies', () => {
  const git = stateOf(setup('git', repo('', [fork, tip]), []));
  assert.deepEqual(choiceOf(git), { kind: 'git', snapshot: 'changed', base: suggested.id });
  git.title = '  ログイン  ';
  git.git = Object.assign({}, git.git, { custom: true, base: MANUAL, manual: ' v1.2 ' });
  assert.deepEqual(choiceOf(git), { kind: 'git', title: 'ログイン', snapshot: 'changed', base: 'v1.2' });
  const ws = stateOf(setup('workspace', null, [repo('a', [tip]), repo('b', [fork, tip])]));
  ws.repos[1] = Object.assign({}, ws.repos[1], { on: true });
  assert.deepEqual(choiceOf(ws), { kind: 'workspace', snapshot: 'changed', repos: [{ path: 'b', base: suggested.id }] }, 'HEAD is not said');
  ws.repos[1] = Object.assign({}, ws.repos[1], { target: ' release ' });
  ws.bulk = { mode: 'last', count: 2 };
  assert.deepEqual(choiceOf(ws).repos, [{ path: 'b', base: 'release~2', target: 'release' }]);
  const raw = stateOf(setup('raw', null, []));
  raw.snapshot = 'full';
  assert.deepEqual(choiceOf(raw), { kind: 'raw' }, 'a directory review keeps everything, and is not asked');
});

test('the choice is held back until every range is said by the server, and something is in them', () => {
  const git = stateOf(setup('git', repo('', [fork, tip]), []));
  assert.equal(problemOf(git), 'ui.setup.range_unchecked', 'not said yet');
  git.git = said(git.git);
  assert.equal(problemOf(git), null);
  git.git = Object.assign({}, git.git, { spanChecking: true });
  assert.equal(problemOf(git), 'ui.setup.range_unchecked', 'asked again');
  git.git = Object.assign({}, git.git, { spanChecking: false, span: null, spanError: 'no such commit' });
  assert.equal(problemOf(git), 'ui.setup.range_invalid');
  git.git = said(git.git, 0, 0);
  assert.equal(problemOf(git), 'ui.setup.nothing_to_review');
  git.git = Object.assign(said(git.git), { custom: true, base: MANUAL, manual: '' });
  assert.equal(problemOf(git), 'ui.setup.base_needed');
  const ws = stateOf(setup('workspace', null, [repo('a', [tip]), repo('b', [tip])]));
  assert.equal(problemOf(ws), 'ui.setup.no_repos_chosen', 'none ticked yet');
  ws.repos[0] = Object.assign(said(ws.repos[0], 0, 0), { on: true });
  ws.repos[1] = Object.assign(said(ws.repos[1], 2, 3), { on: true });
  assert.equal(problemOf(ws), null, 'one with nothing in it is fine, if the others have something');
  assert.deepEqual(totalsOf(ws), { repos: 2, commits: 2, files: 3 });
  ws.repos[1] = Object.assign({}, ws.repos[1], { on: false });
  assert.equal(problemOf(ws), 'ui.setup.nothing_to_review');
  assert.equal(problemOf(stateOf(setup('git', null, []))), 'ui.setup.not_a_repo');
  assert.equal(problemOf(stateOf(setup('raw', null, []))), null);
});

test('a range is said by its commits and files', () => {
  assert.equal(rangeText(span(3, 10)), '3 コミット・10 ファイル');
  assert.equal(rangeText(span(0, 0)), '差分なし');
});

test('the 「リポジトリ」 screen still asks the server about a target it is given', () => {
  const row = rowOf(repo('a', [tip]));
  assert.equal(targetSettled(row), true, 'HEAD needs no asking');
  assert.equal(targetSettled(Object.assign({}, row, { target: 'release', targetChecking: true })), false);
  assert.equal(targetSettled(Object.assign({}, row, { target: 'release', targetPreview: commit('z'.repeat(40), 'r') })), true);
  assert.equal(targetSettled(Object.assign({}, row, { target: '' })), false, 'nothing named');
});

test('a repository named by hand is added once', () => {
  const rows = [rowOf(repo('a', [tip]))];
  const more = withRepo(rows, repo('deep/one', [fork]));
  assert.deepEqual(more.map((r) => r.info.path), ['a', 'deep/one']);
  assert.equal(more[1].on, true, 'named by hand: wanted');
  assert.equal(withRepo(more, repo('a', [tip])), null);
  assert.equal(rows.length, 1, 'the list given is left as it was');
});
