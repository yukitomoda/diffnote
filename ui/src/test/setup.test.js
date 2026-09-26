import test from 'node:test';
import assert from 'node:assert/strict';
import { lib } from '../lib.ts';
import { MANUAL, baseOf, candidateLabel, choiceOf, previewText, problemOf, rowOf, spanText, stateOf, targetSettled, withRepo } from '../screen/setup.ts';

lib.setMessages({
  'ui.setup.name_fork': '{branch} からの分岐点',
  'ui.setup.name_branch': '{branch} の先端',
  'ui.setup.name_head': 'HEAD',
  'ui.setup.name_join': ' / ',
  'ui.setup.preview': '{files} ファイル',
  'ui.setup.span': '{commits} コミット',
  'ui.setup.span_none': '差分なし',
});

const commit = (id, subject) => ({ id, short: id.slice(0, 7), subject });
const candidate = (rev, names, files) => Object.assign(commit(rev.padEnd(40, '0'), 's-' + rev), { rev, names, files });
const repo = (path, candidates) => ({ path, branch: 'feature', head: commit('head'.padEnd(40, 'f'), 'c9'), default_branch: 'main', candidates });
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

test('the screen starts from what the directory looks like, with the best base chosen', () => {
  const one = stateOf(setup('git', repo('', [fork, tip]), []));
  assert.equal(one.kind, 'git');
  assert.equal(one.git.base, 'abc1234');
  assert.equal(one.snapshot, 'changed');
  assert.equal(one.title, '');
  const several = stateOf(Object.assign(setup('workspace', null, [repo('a', [tip]), repo('b', [fork, tip])]), { title: 'T', snapshot: 'full' }));
  assert.equal(several.git, null);
  assert.deepEqual(several.repos.map((r) => [r.info.path, r.on, r.base]), [['a', false, 'main'], ['b', false, 'abc1234']], 'none is in until ticked');
  assert.equal(several.snapshot, 'full');
  assert.equal(several.title, 'T');
  assert.equal(rowOf(repo('c', [])).base, MANUAL, 'nothing offered: typed in');
});

test('what is sent is the kind, the base of each repository kept, and the rest only where it applies', () => {
  const git = stateOf(setup('git', repo('', [fork, tip]), []));
  assert.deepEqual(choiceOf(git), { kind: 'git', snapshot: 'changed', base: 'abc1234' });
  git.title = '  ログイン  ';
  git.git = Object.assign({}, git.git, { base: MANUAL, manual: ' v1.2 ', preview: commit('x'.repeat(40), 'v1.2') });
  assert.deepEqual(choiceOf(git), { kind: 'git', title: 'ログイン', snapshot: 'changed', base: 'v1.2' });
  assert.equal(baseOf(git.git), 'v1.2');
  const ws = stateOf(setup('workspace', null, [repo('a', [tip]), repo('b', [fork, tip])]));
  ws.repos[1] = Object.assign({}, ws.repos[1], { on: true });
  assert.deepEqual(choiceOf(ws), { kind: 'workspace', snapshot: 'changed', repos: [{ path: 'b', base: 'abc1234' }] }, 'HEAD is not said');
  ws.repos[1] = Object.assign({}, ws.repos[1], { target: ' release ', targetPreview: commit('y'.repeat(40), 'r1') });
  assert.deepEqual(choiceOf(ws).repos, [{ path: 'b', base: 'abc1234', target: 'release' }]);
  const raw = stateOf(setup('raw', null, []));
  raw.snapshot = 'full';
  assert.deepEqual(choiceOf(raw), { kind: 'raw' }, 'a directory review keeps everything, and is not asked');
});

test('the choice is held back until it is whole', () => {
  const git = stateOf(setup('git', repo('', [fork, tip]), []));
  assert.equal(problemOf(git), null);
  git.git = Object.assign({}, git.git, { base: MANUAL, manual: '' });
  assert.equal(problemOf(git), 'ui.setup.base_needed');
  git.git = Object.assign({}, git.git, { manual: 'v1', checking: true });
  assert.equal(problemOf(git), 'ui.setup.base_unchecked', 'the server has not said yet');
  git.git = Object.assign({}, git.git, { checking: false, preview: null, error: 'no such commit' });
  assert.equal(problemOf(git), 'ui.setup.base_unchecked', 'nor when it said no');
  git.git = Object.assign({}, git.git, { preview: commit('x'.repeat(40), 'v1'), error: '' });
  assert.equal(problemOf(git), null);
  const ws = stateOf(setup('workspace', null, [repo('a', [tip])]));
  assert.equal(problemOf(ws), 'ui.setup.no_repos_chosen', 'none ticked yet');
  ws.repos[0] = Object.assign({}, ws.repos[0], { on: true });
  assert.equal(problemOf(ws), null);
  assert.equal(problemOf(stateOf(setup('git', null, []))), 'ui.setup.not_a_repo');
  // What is compared up to, when named, must be settled too.
  ws.repos[0] = Object.assign({}, ws.repos[0], { on: true });
  assert.equal(problemOf(ws), null);
  ws.repos[0] = Object.assign({}, ws.repos[0], { target: 'release', targetChecking: true });
  assert.equal(problemOf(ws), 'ui.setup.target_unchecked');
  assert.equal(targetSettled(ws.repos[0]), false);
  ws.repos[0] = Object.assign({}, ws.repos[0], { targetChecking: false, targetError: 'no', targetPreview: null });
  assert.equal(targetSettled(ws.repos[0]), false, 'nor when the server said no');
  ws.repos[0] = Object.assign({}, ws.repos[0], { targetError: '', targetPreview: commit('z'.repeat(40), 'r') });
  assert.equal(targetSettled(ws.repos[0]), true);
  ws.repos[0] = Object.assign({}, ws.repos[0], { target: '' });
  assert.equal(targetSettled(ws.repos[0]), false, 'nothing named');
  ws.repos[0] = Object.assign({}, ws.repos[0], { target: 'HEAD ' });
  assert.equal(targetSettled(ws.repos[0]), true, 'HEAD needs no asking');
  assert.equal(problemOf(stateOf(setup('raw', null, []))), null);
});

test('a repository named by hand is added once', () => {
  const rows = [rowOf(repo('a', [tip]))];
  const more = withRepo(rows, repo('deep/one', [fork]));
  assert.deepEqual(more.map((r) => r.info.path), ['a', 'deep/one']);
  assert.equal(more[1].on, true, 'named by hand: wanted');
  assert.equal(withRepo(more, repo('a', [tip])), null);
  assert.equal(rows.length, 1, 'the list given is left as it was');
});
