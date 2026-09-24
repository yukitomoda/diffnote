// One file of the diff, open or folded away.
import { useContext, useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { server } from '../transport.ts';
import { DiffTable, SplitTable } from './tables.jsx';
import { htmlId } from '../dom.ts';
import { useStore } from '@nanostores/preact';
import { ActionsContext, ComposeContext, OpenedContext } from '../state/contexts.ts';
import { isViewed, seen, toggleViewed } from '../state/viewed.ts';
import { allFolded } from '../state/view.ts';
import { Card } from '../thread/Card.tsx';
import { Composer } from '../thread/Composer.tsx';
import type { Token } from '../model.ts';
import type { RevisionCtx, ShownFile } from '../state/contexts.ts';
import type { Expand } from './tables.tsx';
import { Icon } from '../icon.tsx';

// A file: its own threads, its diff (drawn when it is first opened), and
// the threads that could not be placed in it.
interface FileProps {
  file: ShownFile;
  ctx: RevisionCtx;
}

export function File(props: FileProps) {
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
  var actions = useContext(ActionsContext);
  var details = useRef<HTMLDetailsElement | null>(null);
  // 「すべて開く」「すべて閉じる」: followed when pressed, not when drawn.
  var fold = useStore(allFolded);
  var foldSeen = useRef(fold ? fold.at : 0);
  useEffect(function () {
    if (!fold || fold.at === foldSeen.current) return;
    foldSeen.current = fold.at;
    if (details.current) details.current.open = fold.open;
    if (fold.open) setOpened(true);
  }, [fold]);
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
  var expand: Expand = function (gap, where) {
    const g = (file.gaps || [])[gap];
    const req = g && lib.expandRequest(g, shown[gap] || { top: [], bottom: [] }, where);
    if (!g || !req) return Promise.resolve();
    // Rows of the file are counted by position: what was chosen is let go.
    if (compose && compose!.sel && compose!.sel.path === file.path) compose!.close();
    var get = function (offset: number, count: number): Promise<Token[][]> {
      var held = g.t;
      if (held) return Promise.resolve(held.slice(offset, offset + count));
      var part = function (from: number, left: number, acc: Token[][]): Promise<Token[][]> {
        var take = Math.min(left, 1000);
        return server().get<{ lines: Token[][] }>('/api/files/' + ctx.rev + '/lines?path=' + encodeURIComponent(file.path) + '&from=' + (g.w + from) + '&count=' + take).then(function (res) {
          if (!res.ok || res.lines.length === 0) return acc;
          acc = acc.concat(res.lines);
          return left > take ? part(from + take, left - take, acc) : acc;
        });
      };
      return part(offset, count, []);
    };
    return get(req.offset, req.count).then(function (lines) {
      setRevealed(function (cur) {
        var all: Record<number, Token[]> = Object.assign({}, cur);
        lines.forEach(function (pieces, i) { all[g.w + req.offset + i] = pieces; });
        return all;
      });
    });
  };
  var composing = compose && compose!.scope && compose!.scope.kind === 'file' && compose!.scope.rev === ctx.rev && compose!.scope.path === file.path;
  // A file that was added or deleted as a whole (a binary one too) is tinted.
  var kind = file.status === 'binary' ? file.change : file.status;
  var marks = useStore(seen);
  // A file that was looked at is not shown at all (with its threads): the
  // list at the side says so, and takes it back.
  if (isViewed(file, marks)) return null;
  // What the file's menu offers (the served page only): a thread on the file
  // as a whole, and leaving the file out of what is shown (the review's
  // settings: a file only opened to look at, or brought in by its threads,
  // isn't one the diff shows).
  var canComment = !!compose && (!!file.opened || file.status !== 'context' || mine.length > 0);
  var canIgnore = !!actions && file.status !== 'context';
  return <section class={'diffnote-file' + (kind === 'added' || kind === 'deleted' ? ' diffnote-file--' + kind : '')} id={'r' + ctx.rev + '-file-' + htmlId(file.path)} data-diffnote-file={file.path}>
    <details ref={details} open={startsOpen} onToggle={function (e) { if (e.currentTarget.open && !opened) setOpened(true); }}>
      <summary>
        {<button type="button" class="diffnote-mini--check" data-diffnote-viewed={file.path} title={lib.m('ui.file.viewed_title')}
          aria-label={lib.m('ui.file.viewed_button')}
          onClick={function (e) { e.preventDefault(); e.stopPropagation(); toggleViewed(file); }}><span class="diffnote-tick"><Icon name="check" /></span></button>}
        {(canComment || canIgnore) && <FileMenu
          onComment={canComment ? function () {
            details.current!.open = true;
            setOpened(true);
            compose!.openScope('file', ctx.rev, file.path);
          } : null}
          onIgnore={canIgnore ? function () {
            return actions!.saveSettings({ ignore: lib.withIgnored(ctx.model.settings && ctx.model.settings.ignore, file.path) });
          } : null}
          ignoreBlocked={mine.length > 0} />}
        {file.status !== 'binary' && stat.added + stat.removed > 0 && <span class="diffnote-stat" data-diffnote-stat title={lib.mf('ui.file.stat_title', { added: String(stat.added), removed: String(stat.removed) })}>
          <span class="diffnote-stat__add">+{stat.added}</span> <span class="diffnote-stat__del">−{stat.removed}</span>
          <span class="diffnote-stat__blocks" aria-hidden="true">{lib.diffBlocks(stat.added, stat.removed).map(function (k, i) { return <i key={i} class={'is-' + k}></i>; })}</span>
        </span>}
        <h2>{file.path}{file.status === 'binary' ? (function () {
          var change = lib.messages['ui.binary_change.' + file.change];
          return change ? lib.mf('ui.file.binary_suffix_named', { change: change }) : lib.m('ui.file.binary_suffix_plain');
        })() : ''}{file.status === 'renamed' && (file.old_path
          // Where it was before, said here: the heading is the only place the
          // path it moved from is written, and a folded file shows it too.
          ? <span class="diffnote-file__from" data-diffnote-renamed-from={file.old_path}>{lib.mf('ui.file.renamed_suffix_from', { old: file.old_path })}</span>
          : lib.m('ui.file.renamed_suffix'))}</h2>
        <button type="button" class="diffnote-copy diffnote-icon-button" data-diffnote-copy={file.path} title={lib.m('ui.copy.path_title')} aria-label={lib.m('ui.copy.path_title')}><Icon name="copy" /></button>
        {file.opened && files && <button type="button" class="diffnote-mini" data-diffnote-close title={lib.m('ui.file.close_title')}
          onClick={function (e) {
            e.preventDefault();
            e.stopPropagation();
            if (compose && compose!.scope && compose!.scope.path === file.path) compose!.close();
            if (compose && compose!.sel && compose!.sel.path === file.path) compose!.close();
            files!.close(ctx.rev, file.path);
          }}>{lib.m('ui.file.close_button')}</button>}

      </summary>
      {composing && <div class="diffnote-compose-wrap"><Composer scope="file" where={lib.mf('ui.compose.file_where', { path: file.path })} request={{ scope: 'file', revision: ctx.rev, file: file.path }} /></div>}
      {fileThreads.map(function (id) { return <Card key={id} rev={ctx.rev} thread={ctx.byId[id]} placement={ctx.placements[id]} />; })}
      {missing && <p class="diffnote-file__missing">{lib.m('ui.file.missing_note')}</p>}
      {file.status === 'binary' && <p class="diffnote-file__binary" data-diffnote-binary>{lib.m('ui.file.binary_note')}</p>}
      {/* `view` is the diff with the left-out places folded in, so a file with
          nothing in the diff (one that was only renamed) is a table of one
          place to open. */}
      {opened && view.hunks.length > 0 && (ctx.layout === 'split' ? <SplitTable file={view} ctx={ctx} expand={expand} /> : <DiffTable file={view} ctx={ctx} expand={expand} />)}
      {opened && file.opened && file.next && <div class="diffnote-more-row"><button type="button" class="diffnote-button" data-diffnote-more
        onClick={function (e) { var button = e.currentTarget; button.disabled = true; files!.more(ctx.rev, file.path).then(function () { button.disabled = false; }); }}>{lib.mf('ui.file.more_button', { from: String(file.next), total: String(file.total) })}</button></div>}
      {unplaced.length > 0 && <section class="diffnote-outdated">
        <h3>{lib.m('ui.thread.unplaced_heading')}</h3>
        {unplaced.map(function (id) {
          var p = ctx.placements[id];
          return <div class="diffnote-outdated__entry" key={id}>
            {p.kind === 'unplaced' && p.was.length > 0 && <pre class="diffnote-outdated__snippet">{p.was.join('\n') + '\n'}</pre>}
            <Card rev={ctx.rev} thread={ctx.byId[id]} placement={p} />
          </div>;
        })}
      </section>}
    </details>
  </section>;
}

// The menu on a file's header (left of 確認済み): what can be done to the
// file. Inside the <summary>, so nothing here may fold the file.
interface FileMenuProps {
  onComment: (() => void) | null;
  onIgnore: (() => Promise<{ ok: boolean; error?: string }>) | null;
  /** The file has threads: left out, it would be shown all the same. */
  ignoreBlocked: boolean;
}

function FileMenu(props: FileMenuProps) {
  var _o = useState(false);
  var open = _o[0];
  var setOpen = _o[1];
  var _e = useState('');
  var error = _e[0];
  var setError = _e[1];
  var box = useRef<HTMLSpanElement | null>(null);
  useEffect(function () {
    if (!open) return undefined;
    var away = function (e: MouseEvent) { if (box.current && !box.current.contains(e.target as Node)) setOpen(false); };
    var key = function (e: KeyboardEvent) { if (e.key === 'Escape') setOpen(false); };
    document.addEventListener('mousedown', away);
    document.addEventListener('keydown', key);
    return function () {
      document.removeEventListener('mousedown', away);
      document.removeEventListener('keydown', key);
    };
  }, [open]);
  // (A click here is the menu's: the file doesn't fold or open for it.)
  var own = function (then: () => void) {
    return function (e: MouseEvent) { e.preventDefault(); e.stopPropagation(); then(); };
  };
  return <span class="diffnote-comment__menu diffnote-file__menu" ref={box} onClick={function (e) { e.preventDefault(); e.stopPropagation(); }}>
    <button type="button" class="diffnote-comment__more" data-diffnote-file-menu aria-label={lib.m('ui.file.menu_label')} title={lib.m('ui.file.menu_label')}
      aria-haspopup="true" aria-expanded={open} onClick={own(function () { setOpen(!open); setError(''); })}><Icon name="menu" /></button>
    <span class="diffnote-comment__panel" hidden={!open}>
      {props.onComment && <button type="button" class="diffnote-comment__item" data-diffnote-add="file"
        onClick={own(function () { setOpen(false); props.onComment!(); })}>{lib.m('ui.file.menu_comment')}</button>}
      {props.onIgnore && <button type="button" class="diffnote-comment__item" data-diffnote-ignore-file disabled={props.ignoreBlocked}
        title={props.ignoreBlocked ? lib.m('ui.file.menu_ignore_blocked') : lib.m('ui.file.menu_ignore_title')}
        onClick={own(function () {
          props.onIgnore!().then(function (res) {
            if (res.ok) setOpen(false);
            else setError(res.error || lib.m('ui.save_failed'));
          });
        })}>{lib.m('ui.file.menu_ignore')}</button>}
      {error && <span class="diffnote-error" role="alert">{error}</span>}
    </span>
  </span>;
}
