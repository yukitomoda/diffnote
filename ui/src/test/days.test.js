import test from 'node:test';
import assert from 'node:assert/strict';
import { byRepo, daysOf } from '../screen/days.ts';

const at = (day, hour) => new Date(2026, 8, day, hour).toISOString();
const comment = (day, hour, author, thread) => ({ kind: 'comment', at: at(day, hour), author, thread, comment: thread + '-c' });
const kinds = (day) => day.runs.map((run) => run.map((e) => e.kind + (e.author ? ':' + e.author : '')).join(','));

test('the timeline is read newest first, a day at a time', () => {
  const days = daysOf([
    { kind: 'started', at: at(1, 9) },
    { kind: 'revision', at: at(1, 10), rev: 0, label: '#1 abc', commits: [] },
    comment(3, 9, '共田', 't1'),
    { kind: 'resolved', at: at(3, 18), author: '共田', thread: 't1' },
  ]);
  assert.deepEqual(days.map((d) => d.day), ['2026-09-03', '2026-09-01']);
  assert.deepEqual(kinds(days[0]), ['resolved:共田', 'comment:共田']);
  assert.deepEqual(kinds(days[1]), ['revision', 'started'], 'the review being made is last');
});

test('comments by one person one after another are one run, and nothing else joins', () => {
  const [day] = daysOf([
    comment(2, 9, 'A', 't1'),
    comment(2, 10, 'A', 't2'),
    comment(2, 11, 'B', 't3'),
    comment(2, 12, 'A', 't4'),
    { kind: 'resolved', at: at(2, 13), author: 'A', thread: 't4' },
    { kind: 'resolved', at: at(2, 14), author: 'A', thread: 't1' },
  ]);
  assert.deepEqual(kinds(day), ['resolved:A', 'resolved:A', 'comment:A', 'comment:B', 'comment:A,comment:A']);
  assert.deepEqual(day.runs[4].map((e) => e.thread), ['t2', 't1'], 'a run is newest first too');
});

test('a run does not go on into the day before', () => {
  const days = daysOf([comment(1, 23, 'A', 't1'), comment(2, 0, 'A', 't2')]);
  assert.deepEqual(days.map((d) => d.runs.length), [1, 1]);
});

test('no entries, no days', () => {
  assert.deepEqual(daysOf([]), []);
});

test('the commits a revision brought are grouped by repository, newest first', () => {
  const c = (id, repo) => Object.assign({ id, short: id, author: 'a', at: '', subject: id }, repo ? { repo } : {});
  // By when they were made, where that is known: the model's order is per
  // repository, not across them.
  const dated = (id, repo, day) => Object.assign(c(id, repo), { at: at(day, 12) });
  assert.deepEqual(
    byRepo([dated('a1', 'a', 5), dated('a2', 'a', 7), dated('m1', 'm', 6)]).map((g) => [g.repo, g.commits.map((x) => x.id)]),
    [['a', ['a2', 'a1']], ['m', ['m1']]]);
  assert.deepEqual(
    byRepo([dated('a1', 'a', 5), dated('m1', 'm', 6), dated('a2', 'a', 4)]).map((g) => [g.repo, g.commits.map((x) => x.id)]),
    [['m', ['m1']], ['a', ['a1', 'a2']]]);
  assert.deepEqual(
    byRepo([c('1', 'mobile-app'), c('2', 'backend/repo-a'), c('3', 'mobile-app')]).map((g) => [g.repo, g.commits.map((x) => x.id)]),
    [['mobile-app', ['3', '1']], ['backend/repo-a', ['2']]],
    'the model has them as they were made; the timeline reads newest first');
  assert.deepEqual(byRepo([c('1'), c('2')]).map((g) => [g.repo, g.commits.map((x) => x.id)]), [[null, ['2', '1']]], 'a review of one: one unnamed group');
  assert.deepEqual(byRepo([]), []);
});
