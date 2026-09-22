// One file of the diff, open or folded away.
import { useContext, useMemo, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { transport } from '../transport.ts';
import { DiffTable, SplitTable } from './tables.jsx';
import { htmlId } from '../dom.ts';
import { ComposeContext, OpenedContext, ViewedContext } from '../state/contexts.js';
import { Card } from '../thread/Card.jsx';
import { Composer } from '../thread/Composer.jsx';

// A file: its own threads, its diff (drawn when it is first opened), and
// the threads that could not be placed in it.
export function File(props) {
  var file = props.file;
  var ctx = props.ctx;
  var mine = lib.threadsOfFile(ctx.order, ctx.placements, file.path);
  var fileThreads = mine.filter(function (id) { return ctx.placements[id].kind === 'file'; });
  var unplaced = mine.filter(function (id) { return ctx.placements[id].kind === 'unplaced'; });
  var startsOpen = mine.length > 0 || !!file.opened;
  var files = useContext(OpenedContext);
  var _ = useState(startsOpen);
  var opened = _[0];
  var setOpened = _[1];
  var missing = file.status === 'context' && file.hunks.length === 0;
  var compose = useContext(ComposeContext);
  var details = useRef(null);
  // The lines of the diff's left-out places that have been shown (their pieces,
  // by new line number): kept by number, so they stay when the places change.
  var _g = useState({});
  var revealed = _g[0];
  var setRevealed = _g[1];
  var shown = useMemo(function () { return lib.shownFrom(file.gaps, revealed); }, [file.gaps, revealed]);
  var view = useMemo(function () { return lib.withGaps(ctx.ignoreSpace ? lib.withoutSpaceChanges(file) : file, shown); }, [file, shown, ctx.ignoreSpace]);
  // What the diff of the file adds and removes, as it is shown (so, with white
  // space ignored if it is).
  var stat = lib.diffStat(ctx.ignoreSpace ? lib.withoutSpaceChanges(file) : file);
  var expand = function (gap, where) {
    var g = file.gaps[gap];
    var req = lib.expandRequest(g, shown[gap] || { top: [], bottom: [] }, where);
    if (!req) return Promise.resolve();
    // Rows of the file are counted by position: what was chosen is let go.
    if (compose && compose.sel && compose.sel.path === file.path) compose.close();
    var get = function (offset, count) {
      if (g.t) return Promise.resolve(g.t.slice(offset, offset + count));
      var part = function (from, left, acc) {
        var take = Math.min(left, 1000);
        return transport.get('/api/files/' + ctx.rev + '/lines?path=' + encodeURIComponent(file.path) + '&from=' + (g.w + from) + '&count=' + take).then(function (res) {
          if (!res.ok || res.lines.length === 0) return acc;
          acc = acc.concat(res.lines);
          return left > take ? part(from + take, left - take, acc) : acc;
        });
      };
      return part(offset, count, []);
    };
    return get(req.offset, req.count).then(function (lines) {
      setRevealed(function (cur) {
        var all = Object.assign({}, cur);
        lines.forEach(function (pieces, i) { all[g.w + req.offset + i] = pieces; });
        return all;
      });
    });
  };
  var composing = compose && compose.scope && compose.scope.kind === 'file' && compose.scope.rev === ctx.rev && compose.scope.path === file.path;
  // A file that was added or deleted as a whole (a binary one too) is tinted.
  var kind = file.status === 'binary' ? file.change : file.status;
  var viewed = useContext(ViewedContext);
  // A file that was looked at is not shown at all (with its threads): the
  // list at the side says so, and takes it back.
  if (viewed && viewed.is(file)) return null;
  return <section class={'diffnote-file' + (kind === 'added' || kind === 'deleted' ? ' diffnote-file--' + kind : '')} id={'r' + ctx.rev + '-file-' + htmlId(file.path)} data-diffnote-file={file.path}>
    <details ref={details} open={startsOpen} onToggle={function (e) { if (e.target.open && !opened) setOpened(true); }}>
      <summary>
        {viewed && <button type="button" class="diffnote-mini diffnote-mini--check" data-diffnote-viewed={file.path} title={lib.m('ui.file.viewed_title')}
          onClick={function (e) { e.preventDefault(); e.stopPropagation(); viewed.toggle(file); }}>{lib.m('ui.file.viewed_button')}</button>}
        {file.status !== 'binary' && stat.added + stat.removed > 0 && <span class="diffnote-stat" data-diffnote-stat title={lib.mf('ui.file.stat_title', { added: String(stat.added), removed: String(stat.removed) })}>
          <span class="diffnote-stat__add">+{stat.added}</span> <span class="diffnote-stat__del">−{stat.removed}</span>
          <span class="diffnote-stat__blocks" aria-hidden="true">{lib.diffBlocks(stat.added, stat.removed).map(function (k, i) { return <i key={i} class={'is-' + k}></i>; })}</span>
        </span>}
        <h2>{file.path}{file.status === 'binary' ? (function () {
          var change = lib.messages['ui.binary_change.' + file.change];
          return change ? lib.mf('ui.file.binary_suffix_named', { change: change }) : lib.m('ui.file.binary_suffix_plain');
        })() : ''}{file.status === 'renamed' ? lib.m('ui.file.renamed_suffix') : ''}</h2>
        <button type="button" class="diffnote-copy" data-diffnote-copy={file.path} title={lib.m('ui.copy.path_title')}>{lib.m('ui.copy_button')}</button>
        {file.opened && files && <button type="button" class="diffnote-mini" data-diffnote-close title={lib.m('ui.file.close_title')}
          onClick={function (e) {
            e.preventDefault();
            e.stopPropagation();
            if (compose && compose.scope && compose.scope.path === file.path) compose.close();
            if (compose && compose.sel && compose.sel.path === file.path) compose.close();
            files.close(ctx.rev, file.path);
          }}>{lib.m('ui.file.close_button')}</button>}
        {compose && (file.opened || file.status !== 'context' || mine.length > 0) && <button type="button" class="diffnote-mini" data-diffnote-add="file" title={lib.m('ui.file.add_comment_title')}
          onClick={function (e) {
            e.preventDefault();
            e.stopPropagation();
            details.current.open = true;
            setOpened(true);
            compose.openScope('file', ctx.rev, file.path);
          }}>{lib.m('ui.file.add_comment_button')}</button>}
      </summary>
      {composing && <div class="diffnote-compose-wrap"><Composer scope="file" where={lib.mf('ui.compose.file_where', { path: file.path })} request={{ scope: 'file', revision: ctx.rev, file: file.path }} /></div>}
      {fileThreads.map(function (id) { return <Card key={id} rev={ctx.rev} thread={ctx.byId[id]} placement={ctx.placements[id]} />; })}
      {missing && <p class="diffnote-file__missing">{lib.m('ui.file.missing_note')}</p>}
      {file.status === 'binary' && <p class="diffnote-file__binary" data-diffnote-binary>{lib.m('ui.file.binary_note')}</p>}
      {opened && file.hunks.length > 0 && (ctx.layout === 'split' ? <SplitTable file={view} ctx={ctx} expand={expand} /> : <DiffTable file={view} ctx={ctx} expand={expand} />)}
      {opened && file.opened && file.next && <div class="diffnote-more-row"><button type="button" class="diffnote-button" data-diffnote-more
        onClick={function (e) { e.target.disabled = true; files.more(ctx.rev, file.path).then(function () { e.target.disabled = false; }); }}>{lib.mf('ui.file.more_button', { from: String(file.next), total: String(file.total) })}</button></div>}
      {unplaced.length > 0 && <section class="diffnote-outdated">
        <h3>{lib.m('ui.thread.unplaced_heading')}</h3>
        {unplaced.map(function (id) {
          var p = ctx.placements[id];
          return <div class="diffnote-outdated__entry" key={id}>
            {p.was.length > 0 && <pre class="diffnote-outdated__snippet">{p.was.join('\n') + '\n'}</pre>}
            <Card rev={ctx.rev} thread={ctx.byId[id]} placement={p} />
          </div>;
        })}
      </section>}
    </details>
  </section>;
}
