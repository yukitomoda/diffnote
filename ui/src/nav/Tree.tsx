// The other files: everything the review holds, as a tree.
import { Fragment } from 'preact';
import { useContext, useEffect, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { server } from '../transport.ts';
import type { TreeAnswer, TreeEntry } from '../model.ts';
import { OpenedContext } from '../state/contexts.ts';

// The entries of a directory of the files that could be opened (read from
// the server when shown: a directory of thousands costs nothing until then).
interface ListProps {
  rev: number;
  dir: string;
  query: string;
  onOpen(path: string): void;
}

function TreeList(props: ListProps) {
  var _ = useState<TreeAnswer | null>(null);
  var data = _[0];
  var setData = _[1];
  useEffect(function () {
    var stale = false;
    setData(null);
    server().get<{ entries: TreeEntry[]; message: string | null; note: string | null; more: number }>('/api/files/' + props.rev + '/tree?json=1&dir=' + encodeURIComponent(props.dir) + '&q=' + encodeURIComponent(props.query)).then(function (res) {
      if (!stale) setData(res);
    });
    return function () { stale = true; };
  }, [props.rev, props.dir, props.query]);
  if (!data) return <p class="diffnote-tree__empty">{lib.m('ui.tree.loading')}</p>;
  if (!data.ok) return <p class="diffnote-tree__empty">{data.error || lib.m('ui.tree.load_failed')}</p>;
  return <Fragment>
    {data.message && <p class="diffnote-tree__empty">{data.message}</p>}
    {data.entries.length > 0 && <ul class="diffnote-tree__list">
      {data.entries.map(function (e: TreeEntry) {
        return e.kind === 'dir'
          ? <li key={e.path}><TreeDir rev={props.rev} entry={e} onOpen={props.onOpen} /></li>
          : <li key={e.path}><button type="button" class="diffnote-tree__file" data-diffnote-open={e.path} title={e.path}
              onClick={function () { props.onOpen(e.path); }}>{e.name}</button></li>;
      })}
    </ul>}
    {data.note && <p class="diffnote-tree__empty">{data.note}</p>}
    {data.more > 0 && <p class="diffnote-tree__empty">{lib.mf('ui.tree.more_note', { n: String(data.more) })}</p>}
  </Fragment>;
}

interface DirProps {
  rev: number;
  entry: TreeEntry & { kind: 'dir' };
  onOpen(path: string): void;
}

function TreeDir(props: DirProps) {
  var _ = useState(false);
  var shown = _[0];
  var setShown = _[1];
  var e = props.entry;
  return <details class="diffnote-tree__dir" data-diffnote-dir={e.path} onToggle={function (ev) { if (ev.currentTarget.open) setShown(true); }}>
    <summary>{e.name}/ <span class="diffnote-tree__count">{e.count}</span></summary>
    {shown && <TreeList rev={props.rev} dir={e.path} query="" onOpen={props.onOpen} />}
  </details>;
}

// "Other files": what the review has (or, next to the repository, the commit
// has) that the diff doesn't show. Opening one shows it, records nothing.
interface TreeProps {
  rev: number;
}

export function Tree(props: TreeProps) {
  var files = useContext(OpenedContext);
  var _s = useState(false);
  var shown = _s[0];
  var setShown = _s[1];
  var _q = useState('');
  var typed = _q[0];
  var setTyped = _q[1];
  var _d = useState('');
  var query = _d[0];
  var setQuery = _d[1];
  var _e = useState('');
  var error = _e[0];
  var setError = _e[1];
  useEffect(function () {
    var t = setTimeout(function () { setQuery(typed); }, 250);
    return function () { clearTimeout(t); };
  }, [typed]);
  var open = function (path: string) {
    setError('');
    files!.open(props.rev, path).then(function (res) { if (!res.ok) setError(res.error || lib.m('ui.file.open_failed')); });
  };
  return <details class="diffnote-side diffnote-side--quiet" data-diffnote-tree onToggle={function (e) { if (e.target === e.currentTarget && e.currentTarget.open) setShown(true); }}>
    <summary>{lib.m('ui.tree.others_summary')}</summary>
    <div class="diffnote-tree">
      <input type="search" class="diffnote-tree__search" placeholder={lib.m('ui.tree.search_placeholder')} aria-label={lib.m('ui.tree.search_label')} value={typed}
        onInput={function (e) { setTyped(e.currentTarget.value); }} />
      <div data-diffnote-tree-list>{shown && <TreeList rev={props.rev} dir="" query={query} onOpen={open} />}</div>
      {error && <p class="diffnote-error">{error}</p>}
    </div>
  </details>;
}
