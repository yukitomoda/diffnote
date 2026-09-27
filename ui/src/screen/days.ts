// The timeline's days and runs, apart from how they are drawn (Timeline.tsx),
// so that the order and the folding can be tested without a page.
import { lib } from '../lib.ts';
import type { TimelineCommit, TimelineEntry } from '../model.ts';

/** A run of entries on one day, with the entries of a run by one author on
 * one kind of thing folded into a single line. */
export interface Day {
  day: string;
  runs: TimelineEntry[][];
}

/**
 * The days, and within each the runs to draw -- newest first, which is not
 * the order the model carries them in (that is the log's own, oldest first).
 */
export function daysOf(entries: TimelineEntry[]): Day[] {
  var days: Day[] = [];
  entries.slice().reverse().forEach(function (entry) {
    var day = lib.formatDay(entry.at);
    var last = days[days.length - 1];
    if (!last || last.day !== day) {
      last = { day: day, runs: [] };
      days.push(last);
    }
    // Comments by one author, one after another, are one line with a count:
    // a pass over a change writes several at once, and a day of them would
    // otherwise be a wall.
    var run = last.runs[last.runs.length - 1];
    var joins = run
      && entry.kind === 'comment'
      && run[0].kind === 'comment'
      && run[0].author === entry.author;
    if (joins) run.push(entry);
    else last.runs.push([entry]);
  });
  return days;
}

/** The commits a revision brought, by repository (a review of several),
 * newest first as the rest of the timeline is (the model carries them as
 * they were made; two made at the same moment keep that order), the
 * repositories in the order their newest come: one group, unnamed, for a
 * review of one. */
export function byRepo(commits: TimelineCommit[]): { repo: string | null; commits: TimelineCommit[] }[] {
  var groups: { repo: string | null; commits: TimelineCommit[] }[] = [];
  var newestFirst = commits.map(function (c, i) { return { c: c, i: i, t: Date.parse(c.at) || 0 }; });
  newestFirst.sort(function (x, y) { return y.t - x.t || y.i - x.i; });
  newestFirst.map(function (e) { return e.c; }).forEach(function (c) {
    var repo = c.repo || null;
    var group = groups.filter(function (g) { return g.repo === repo; })[0];
    if (!group) {
      group = { repo: repo, commits: [] };
      groups.push(group);
    }
    group.commits.push(c);
  });
  return groups;
}
