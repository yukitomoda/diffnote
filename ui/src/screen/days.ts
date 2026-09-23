// The timeline's days and runs, apart from how they are drawn (Timeline.tsx),
// so that the order and the folding can be tested without a page.
import { lib } from '../lib.ts';
import type { TimelineEntry } from '../model.ts';

/** A run of entries on one day, with the entries of a run by one author on
 * one kind of thing folded into a single line. */
export interface Day {
  day: string;
  runs: TimelineEntry[][];
}

/**
 * The days, and within each the runs to draw -- newest first, which is not
 * the order the model carries them in (that is the log's own, oldest first).
 * The commits a revision brought stay in the order they were made: the
 * stream is read from the top, a series of commits from its beginning.
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
