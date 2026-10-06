// A repository's history, drawn as a graph, newest first, to choose the two
// commits a review compares -- their snapshots, the one it compares from and
// the one it compares up to: on the first screen, beside the form. Where each
// commit and line goes is the server's to say (`GraphRow`, `src/setup.rs`);
// this draws it, marks the range the form has, and offers each row's commit
// as an end of it.
import { useEffect, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { server } from '../transport.ts';
import type { GraphRef, GraphRow, SetupGraph, SetupSpan } from '../model.ts';

/** How tall a row is, and how wide a column of lines (px). */
var ROW = 26;
var LANE = 12;

/** Where a column's line runs, across. */
function across(lane: number): number {
  return lane * LANE + LANE / 2 + 2;
}

/** A line from one point to another, bent where it changes column. */
function bend(x1: number, y1: number, x2: number, y2: number): string {
  if (x1 === x2) return 'M' + x1 + ' ' + y1 + 'V' + y2;
  var mid = (y1 + y2) / 2;
  return 'M' + x1 + ' ' + y1 + 'C' + x1 + ' ' + mid + ' ' + x2 + ' ' + mid + ' ' + x2 + ' ' + y2;
}

/** The names a commit goes by, in the order they are offered as where a
 * review ends: `HEAD`, the local branches, the tags, the remote ones. */
var KIND_ORDER: GraphRef['kind'][] = ['head', 'branch', 'tag', 'remote'];
function namesOf(row: GraphRow): GraphRef[] {
  return (row.refs || []).slice().sort(function (a, b) { return KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind); });
}

/** Every name of a graph, for the field where a review ends to offer. */
export function graphNames(graph: SetupGraph): string[] {
  var seen: Record<string, boolean> = {};
  var out: string[] = [];
  graph.rows.forEach(function (r) {
    namesOf(r).forEach(function (n) { if (!seen[n.name]) { seen[n.name] = true; out.push(n.name); } });
  });
  return out;
}

/** The lines and the dot of one row (see `GraphRow`). */
function Lines(props: { row: GraphRow; lanes: number }) {
  var r = props.row;
  var mid = ROW / 2;
  var width = Math.max(1, props.lanes) * LANE + 4;
  var head = (r.refs || []).some(function (n) { return n.kind === 'head'; });
  return <svg class="diffnote-graph__lines" width={width} height={ROW} viewBox={'0 0 ' + width + ' ' + ROW} aria-hidden="true">
    {r.through.map(function (j) {
      return <path key={'t' + j} d={bend(across(j), 0, across(j), ROW)} stroke={lib.color(j)} />;
    })}
    {r.up.map(function (j) {
      return <path key={'u' + j} d={bend(across(j), 0, across(r.lane), mid)} stroke={lib.color(j)} />;
    })}
    {r.down.map(function (k) {
      return <path key={'d' + k} d={bend(across(r.lane), mid, across(k), ROW)} stroke={lib.color(k)} />;
    })}
    <circle cx={across(r.lane)} cy={mid} r={head ? 5 : 4} fill={head ? 'var(--diffnote-color-bg)' : lib.color(r.lane)} stroke={lib.color(r.lane)} stroke-width={head ? 2.5 : 1} />
  </svg>;
}

export interface GraphProps {
  /** The repository (its path; empty: the one). */
  repo: string;
  /** What the form's range takes in, as the server said: which commits are
   * in it, and where it starts and ends. */
  span: SetupSpan | null;
  /** Whether where it ends can be chosen (a review of one repository goes
   * to `HEAD`, each time). */
  canTarget: boolean;
  /** A commit chosen as where the review compares from: its snapshot is
   * what the target's is compared with. */
  onFrom(base: string): void;
  /** Where the review ends: a name (`HEAD`, a branch: it moves on, and
   * each review goes as far as it has got), or a commit (it stays). */
  onTo(target: string): void;
  /** The names the graph has, once it is read. */
  onNames?(names: string[]): void;
}

export function CommitGraph(props: GraphProps) {
  var _g = useState<SetupGraph | null>(null);
  var graph = _g[0];
  var setGraph = _g[1];
  var _e = useState('');
  var error = _e[0];
  var setError = _e[1];
  var _l = useState(false);
  var loading = _l[0];
  var setLoading = _l[1];
  // The row whose 「ここまで」 asks which of its names (or the commit) it is.
  var _p = useState<string | null>(null);
  var picking = _p[0];
  var setPicking = _p[1];
  useEffect(function () {
    if (!picking) return undefined;
    var away = function (e: MouseEvent) {
      if (!(e.target instanceof Element && e.target.closest('[data-diffnote-graph-names]'))) setPicking(null);
    };
    var key = function (e: KeyboardEvent) { if (e.key === 'Escape') setPicking(null); };
    document.addEventListener('mousedown', away);
    document.addEventListener('keydown', key);
    return function () {
      document.removeEventListener('mousedown', away);
      document.removeEventListener('keydown', key);
    };
  }, [picking]);
  var latest = useRef(0);
  var load = function (limit: number | null) {
    var n = ++latest.current;
    setLoading(true);
    server().get<{ graph: SetupGraph }>('api/setup/graph?repo=' + encodeURIComponent(props.repo) + (limit ? '&limit=' + limit : '')).then(function (res) {
      if (latest.current !== n) return;
      setLoading(false);
      if (res.ok) {
        setGraph(res.graph);
        setError('');
        if (props.onNames) props.onNames(graphNames(res.graph));
      }
      else setError(res.error);
    });
  };
  useEffect(function () { setGraph(null); load(null); }, [props.repo]);
  var span = props.span;
  var inRange: Record<string, boolean> = {};
  (span ? span.ids : []).forEach(function (id) { inRange[id] = true; });
  return <div class="diffnote-graph" data-diffnote-graph={props.repo}>
    <p class="diffnote-graph__head">
      <strong>{props.repo ? lib.mf('ui.setup.graph_heading', { repo: props.repo }) : lib.m('ui.setup.graph_heading_one')}</strong>
      <small>{lib.m(props.canTarget ? 'ui.setup.graph_hint' : 'ui.setup.graph_hint_from')}</small>
    </p>
    {error && <p class="diffnote-error" role="alert">{error}</p>}
    {!graph && !error && <p class="diffnote-screen__note">{lib.m('ui.setup.graph_loading')}</p>}
    {graph && <ol class="diffnote-graph__rows">
      {graph.rows.map(function (r) {
        var from = !!span && span.from === r.id;
        var to = !!span && span.id === r.id;
        return <li key={r.id} class={'diffnote-graph__row' + (inRange[r.id] ? ' is-in' : '') + (from ? ' is-from' : '') + (to ? ' is-to' : '')}
          data-diffnote-graph-commit={r.id} data-diffnote-graph-in={inRange[r.id] ? '' : undefined}>
          <Lines row={r} lanes={graph!.lanes} />
          <span class="diffnote-graph__text" title={r.subject + '\n' + r.author + ' · ' + lib.formatRecorded(r.at)}>
            {(r.refs || []).map(function (n) {
              return <span key={n.kind + n.name} class={'diffnote-graph__ref is-' + n.kind} data-diffnote-graph-ref={n.name}>{n.name}</span>;
            })}
            <code>{r.short}</code>
            <span class="diffnote-graph__subject">{r.subject}</span>
          </span>
          <span class="diffnote-graph__at">{lib.formatRecorded(r.at)}</span>
          {from && <span class="diffnote-graph__mark" data-diffnote-graph-from-mark>{lib.m('ui.setup.graph_from_mark')}</span>}
          {to && <span class="diffnote-graph__mark" data-diffnote-graph-to-mark>{lib.m('ui.setup.graph_to_mark')}</span>}
          <span class="diffnote-graph__actions">
            <button type="button" class="diffnote-graph__pick" data-diffnote-graph-from title={lib.m('ui.setup.graph_from_title')}
              onClick={function () { props.onFrom(r.id); }}>{lib.m('ui.setup.graph_from')}</button>
            {props.canTarget && <button type="button" class="diffnote-graph__pick" data-diffnote-graph-to title={lib.m('ui.setup.graph_to_title')}
              onClick={function () {
                // Which it is to be, said before it is chosen: one of its
                // names (which moves on with what is done after) or the
                // commit (which stays) -- the commit alone, if it has none.
                setPicking(picking === r.id ? null : r.id);
              }}>{lib.m('ui.setup.graph_to')}</button>}
          </span>
          {picking === r.id && <span class="diffnote-graph__names" role="menu" data-diffnote-graph-names>
            {namesOf(r).map(function (n) {
              return <button type="button" role="menuitem" key={n.kind + n.name} class="diffnote-comment__item" data-diffnote-graph-to-name={n.name}
                title={lib.m('ui.setup.graph_to_name_title')}
                onClick={function () { setPicking(null); props.onTo(n.name); }}>
                <span class={'diffnote-graph__ref is-' + n.kind}>{n.name}</span>
                <small>{lib.m('ui.setup.graph_to_name_' + n.kind)}</small>
              </button>;
            })}
            <button type="button" role="menuitem" class="diffnote-comment__item" data-diffnote-graph-to-commit title={lib.m('ui.setup.graph_to_commit_title')}
              onClick={function () { setPicking(null); props.onTo(r.id); }}>
              <code>{r.short}</code><small>{lib.m('ui.setup.graph_to_commit')}</small>
            </button>
          </span>}
        </li>;
      })}
    </ol>}
    {graph && graph.more && <button type="button" class="diffnote-button diffnote-graph__more" data-diffnote-graph-more disabled={loading}
      onClick={function () { load(graph!.rows.length + 100); }}>{lib.m('ui.setup.graph_more')}</button>}
  </div>;
}
