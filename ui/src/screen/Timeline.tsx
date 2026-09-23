// タイムライン: what happened to the review, newest first.
//
// Everything here is the review's own log, read in order (`src/html/viewmodel.rs`
// puts it together): nothing is recorded for the timeline's sake. Because a
// comment that is edited or deleted rewrites that log, this says what the log
// holds now -- it is not an account of every change ever made.
import { EMOJI } from '../emoji.ts';
import { Icon } from '../icon.tsx';
import type { IconName } from '../icon.tsx';
import { lib } from '../lib.ts';
import { openRevision } from '../state/route.ts';
import type { CommentData, Placement, TimelineCommit, TimelineEntry, ViewModel } from '../model.ts';

export interface TimelineProps {
  model: ViewModel;
  /** Going to the comment an entry is about, and where that comment is. */
  onShow(thread: string): void;
  placementOf(thread: string): Placement | undefined;
}

/** A run of entries on one day, with the entries of a run by one author on
 * one kind of thing folded into a single line. */
interface Day {
  day: string;
  runs: TimelineEntry[][];
}

/**
 * The days, and within each the runs to draw -- newest first, which is not
 * the order the model carries them in (that is the log's own, oldest first).
 * The commits a revision brought stay in the order they were made: the
 * stream is read from the top, a series of commits from its beginning.
 */
function daysOf(entries: TimelineEntry[]): Day[] {
  var days: Day[] = [];
  entries.slice().reverse().forEach(function (entry) {
    var day = lib.formatDay(entry.at);
    var last = days[days.length - 1];
    if (!last || last.day !== day) {
      last = { day: day, runs: [] };
      days.push(last);
    }
    // Comments by one author, one after another, are one line with a count:
    // an `edit` session writes several at once, and a day of them would
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

var MARK: Record<TimelineEntry['kind'], IconName> = {
  started: 'start',
  revision: 'commit',
  comment: 'comment',
  resolved: 'done',
  reopened: 'undone',
};

/** The comment an entry is about, wherever it is in the review. */
function commentOf(model: ViewModel, id: string): CommentData | null {
  for (var i = 0; i < model.threads.length; i++) {
    var found = model.threads[i].comments.filter(function (c) { return c.id === id; })[0];
    if (found) return found;
  }
  return null;
}

function Commit(props: { commit: TimelineCommit }) {
  var c = props.commit;
  var said = lib.splitTrailers(c.body);
  var files = c.files || [];
  var more = !!said.body || said.trailers.length > 0 || files.length > 0;
  return <li class="diffnote-timeline__commit" data-diffnote-commit={c.id}>
    <details>
      <summary>
        <code class="diffnote-timeline__id">{c.short}</code>
        <span class="diffnote-timeline__subject">{c.subject}</span>
        <span class="diffnote-timeline__who">{c.author}</span>
        <span class="diffnote-timeline__at">{lib.formatClock(c.at)}</span>
        <Icon name="down" class="diffnote-timeline__chevron" />
      </summary>
      {more && <div class="diffnote-timeline__message">
        {said.body && <p class="diffnote-timeline__body">{said.body}</p>}
        {files.length > 0 && <ul class="diffnote-timeline__files" data-diffnote-commit-files>
          {files.map(function (f, i) {
            return <li key={i}><span class={'diffnote-timeline__status is-' + f.status}>{lib.m('ui.timeline.status_' + f.status)}</span>
              {' '}<code>{f.old_path ? f.old_path + ' → ' + f.path : f.path}</code></li>;
          })}
        </ul>}
        {said.trailers.length > 0 && <details class="diffnote-timeline__trailers">
          <summary>{lib.mf('ui.timeline.trailers', { n: String(said.trailers.length) })}</summary>
          <p>{said.trailers.join('\n')}</p>
        </details>}
      </div>}
    </details>
  </li>;
}

export function TimelinePane(props: TimelineProps) {
  var model = props.model;
  var entries = model.timeline || [];
  var days = daysOf(entries);

  /** What one entry says, as a line: the words, and where they lead. */
  var line = function (entry: TimelineEntry) {
    if (entry.kind === 'started') return <span>{lib.m('ui.timeline.started')}</span>;
    if (entry.kind === 'revision') {
      return <button type="button" class="diffnote-timeline__go" data-diffnote-timeline-revision={entry.rev}
        onClick={function () { openRevision(entry.rev); }}>{lib.mf('ui.timeline.revision', { label: entry.label })}</button>;
    }
    var where = lib.shortLocation(props.placementOf(entry.thread));
    var go = <button type="button" class="diffnote-timeline__go" data-diffnote-timeline-thread={entry.thread}
      onClick={function () { props.onShow(entry.thread); }}>{where || lib.m('ui.timeline.somewhere')}</button>;
    if (entry.kind === 'resolved' || entry.kind === 'reopened') {
      return <span>{lib.mf('ui.timeline.' + entry.kind, { author: entry.author })}{' '}{go}</span>;
    }
    var comment = commentOf(model, entry.comment);
    var said = comment ? lib.withShortcodes(EMOJI, lib.preview(comment.doc)) : '';
    return <span>{lib.mf(entry.reply ? 'ui.timeline.reply' : 'ui.timeline.comment', { author: entry.author })}{' '}{go}
      {said && <span class="diffnote-timeline__said">{said}</span>}</span>;
  };

  var one = function (entry: TimelineEntry, key: string | number) {
    return <li key={key} class="diffnote-timeline__entry" data-diffnote-timeline={entry.kind}>
      <Icon name={MARK[entry.kind]} class="diffnote-timeline__mark" />
      <div class="diffnote-timeline__what">
        {line(entry)}
        {entry.kind === 'revision' && (entry.commits || []).length > 0 && <ul class="diffnote-timeline__commits">
          {entry.commits!.map(function (c) { return <Commit key={c.id} commit={c} />; })}
        </ul>}
      </div>
      <span class="diffnote-timeline__at">{lib.formatClock(entry.at)}</span>
    </li>;
  };

  return <div data-diffnote-timeline-pane>
    <h2>{lib.m('ui.timeline.heading')}</h2>
    <p class="diffnote-screen__note">{lib.m('ui.timeline.note')}</p>
    {entries.length === 0
      ? <p class="diffnote-screen__note">{lib.m('ui.timeline.empty')}</p>
      : days.map(function (day) {
          return <section key={day.day} class="diffnote-timeline__day" data-diffnote-timeline-day={day.day}>
            <h3>{day.day}</h3>
            <ol class="diffnote-timeline">
              {day.runs.map(function (run, i) {
                if (run.length === 1) return one(run[0], i);
                // A run of comments: one line, opening onto them. The
                // line is timed by the newest of them, which is the first.
                var first = run[0];
                return <li key={i} class="diffnote-timeline__entry" data-diffnote-timeline="comments">
                  <Icon name="comment" class="diffnote-timeline__mark" />
                  <details class="diffnote-timeline__what" data-diffnote-timeline-run>
                    <summary><Icon name="down" class="diffnote-timeline__chevron" />{' '}{lib.mf('ui.timeline.comments', {
                      author: first.kind === 'comment' ? first.author : '',
                      n: String(run.length),
                    })}</summary>
                    <ol class="diffnote-timeline">{run.map(function (e, j) { return one(e, j); })}</ol>
                  </details>
                  <span class="diffnote-timeline__at">{lib.formatClock(first.at)}</span>
                </li>;
              })}
            </ol>
          </section>;
        })}
  </div>;
}
