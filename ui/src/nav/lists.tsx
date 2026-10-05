// The lists beside the diff: the files, and the threads.
import { useContext, useEffect, useRef, useState } from 'preact/hooks';
import { EMOJI } from '../emoji.ts';
import { lib } from '../lib.ts';
import { interact } from '../interact.ts';
import { htmlId } from '../dom.ts';
import { useStore } from '@nanostores/preact';
import { ActionsContext, LinksContext } from '../state/contexts.ts';
import { isViewed, seen, toggleViewed } from '../state/viewed.ts';
import type { ListCtx } from '../state/contexts.ts';
import type { FileTreeNode } from '../lib.ts';
import { Icon } from '../icon.tsx';

interface ListProps {
  ctx: ListCtx;
}

/** What a right click in the file list is on: a directory or a file to leave
 * out, and where to show what can be done to it. */
interface Target {
  path: string;
  dir: boolean;
  /** A file with threads: left out, it would be shown all the same. */
  blocked: boolean;
  x: number;
  y: number;
}

// The files, as a tree of their directories (see `lib.fileTree`). Each file
// says how many threads are on it, and, at the right, whether it was looked at.
// On the served page, a right click on a directory (each of a joined row's by
// itself) or a file offers to leave it out (the review's settings).
export function FileList(props: ListProps) {
  var ctx = props.ctx;
  var marks = useStore(seen);
  var revId = ctx.model.revisions[ctx.rev].id;
  var links = useContext(LinksContext);
  var actions = useContext(ActionsContext);
  var _t = useState<Target | null>(null);
  var target = _t[0];
  var setTarget = _t[1];
  // The files of the diff (not those opened to look at) are what is counted.
  var files = ctx.diffFiles;
  var byPath: Record<string, (typeof ctx.revision.files)[number]> = {};
  ctx.revision.files.forEach(function (f) { byPath[f.path] = f; });
  var tree = lib.fileTree(ctx.revision.files.map(function (f) { return f.path; }));
  // What the review leaves out (its settings): named here, not shown.
  var ignored = ctx.revision.ignored || [];
  // Each directory of a row by itself, for a right click to name.
  var segments = function (node: FileTreeNode) {
    return node.dirs.map(function (d) {
      // (Where a right click offers something, it says so: see `onMenu`.)
      return <span key={d.path} data-diffnote-tree-dir={d.path} class={target && target.dir && target.path === d.path ? 'is-target' : undefined}
        title={actions ? lib.mf('ui.tree.dir_menu_title', { pattern: lib.ignoreLine(d.path) }) : undefined}>{d.label}</span>;
    });
  };
  var onMenu = function (e: MouseEvent) {
    if (!actions) return;
    var at = e.target as Element;
    var dir = at.closest('[data-diffnote-tree-dir]');
    var link = dir ? null : at.closest('[data-diffnote-file-link]');
    var path = dir ? dir.getAttribute('data-diffnote-tree-dir') : link && link.getAttribute('data-diffnote-file-link');
    if (!path) return;
    // A file only opened to look at, or brought in by its threads, isn't one
    // the diff shows: nothing to leave out (the browser's own menu, then).
    if (!dir && (!byPath[path] || byPath[path].status === 'context')) return;
    e.preventDefault();
    setTarget({
      path: path,
      dir: !!dir,
      blocked: !dir && lib.threadsOfFile(ctx.order, ctx.placements, path).length > 0,
      x: e.clientX,
      y: e.clientY,
    });
  };
  var file = function (node: FileTreeNode) {
    var f = byPath[node.path!];
    var done = isViewed(f, marks, revId);
    var ids = lib.threadsOfFile(ctx.order, ctx.placements, f.path);
    // The threads that are shown: resolved ones don't count while hidden.
    var n = ids.filter(function (id) {
      return !(ctx.hideResolved && ctx.byId[id].resolved);
    }).length;
    // What is left open in a file that was looked at.
    var open = ids.filter(function (id) { return !ctx.byId[id].resolved; }).length;
    return <li key={f.path} class={'diffnote-filelist__file' + (done ? ' is-viewed' : '')}>
      <a href={'#r' + ctx.rev + '-file-' + htmlId(f.path)} data-diffnote-file-link={f.path} title={f.path}
        onClick={function (e) {
          // A file that was looked at comes back; one that was folded opens; and it is marked.
          e.preventDefault();
          links.go({ kind: 'file', path: f.path });
        }}>{segments(node)}<span data-diffnote-tree-file={f.path}
          title={actions && f.status !== 'context' ? lib.mf('ui.tree.file_menu_title', { path: f.path }) : undefined}>{node.label.slice(node.dirs.map(function (d) { return d.label; }).join('').length)}</span></a>
      {done
        ? open > 0 && <span class="diffnote-badge" data-diffnote-open-count title={lib.mf('ui.thread.open_count_title', { n: String(open) })}>{open}</span>
        : n > 0 && <span class="diffnote-badge">{n}</span>}
      <button type="button" class="diffnote-check" data-diffnote-check={f.path} aria-pressed={done}
        title={done ? lib.m('ui.file.unmark_viewed_title') : lib.m('ui.file.mark_viewed_title')} onClick={function () {
          // (Marked, the file leaves the page: see `interact.leaving`.)
          var then = done ? function () {} : interact.leaving(ctx.rev, f.path);
          toggleViewed(f, revId);
          then();
        }}><span class={'diffnote-tick' + (done ? ' is-on' : '')}><Icon name="check" /></span></button>
    </li>;
  };
  // The files left out, as a tree like the files above: named, nothing to press.
  var names = function (nodes: FileTreeNode[]): preact.ComponentChildren {
    return nodes.map(function (node) {
      if (node.path != null) return <li key={node.path} title={node.path} data-diffnote-ignored-file={node.path}>{node.label}</li>;
      return <li key={'dir:' + node.label} class="diffnote-filelist__dir">
        <span class="diffnote-filelist__dirname">{node.label}</span>
        <ul>{names(node.children)}</ul>
      </li>;
    });
  };
  var rows = function (nodes: FileTreeNode[]): preact.ComponentChildren {
    return nodes.map(function (node) {
      if (node.path != null) return file(node);
      return <li key={'dir:' + node.label} class="diffnote-filelist__dir">
        <span class="diffnote-filelist__dirname">{segments(node)}</span>
        <ul>{rows(node.children)}</ul>
      </li>;
    });
  };
  // A review of several repositories: the files by repository, each a group
  // that folds, in the repository's color, with what it is compared from and
  // up to and how many of its files were looked at. The files are named from
  // the repository's directory, which is never joined to them.
  var repos = ctx.revision.repos || [];
  var groups = function (): preact.ComponentChildren {
    var all = ctx.revision.files;
    var known: Record<string, boolean> = {};
    var shown = repos.map(function (r) {
      known[r.path] = true;
      var mine = all.filter(function (f) { return f.repo === r.path; });
      if (!mine.length) return null;
      var counted = files.filter(function (f) { return f.repo === r.path; });
      var done = counted.filter(function (f) { return isViewed(f, marks, revId); }).length;
      var dir = r.path + '/';
      return <li key={'repo:' + r.path} class="diffnote-filelist__repo" data-diffnote-repo-group={r.path} style={'--diffnote-repo-color:' + lib.repoColor(r.color)}>
        <details open>
          <summary class="diffnote-filelist__reponame" title={lib.mf('ui.repolist.group_title', { path: r.path, target: r.target, base: r.base, head: r.head })}>
            {/* (First, so that it floats beside the name's line.) */}
            {counted.length > 0 && <span class="diffnote-filelist__repocount" data-diffnote-repo-viewed title={lib.m('ui.tree.viewed_count_title')}>{done}/{counted.length}</span>}
            <span class="diffnote-repo-dot" aria-hidden="true"></span>
            <span class="diffnote-filelist__repopath">
              <span data-diffnote-tree-dir={dir} class={target && target.dir && target.path === dir ? 'is-target' : undefined}
                title={actions ? lib.mf('ui.tree.dir_menu_title', { pattern: lib.ignoreLine(dir) }) : undefined}>{r.path}</span>
              <small class="diffnote-filelist__reporange" data-diffnote-repo-range>{r.target} {r.base}→{r.head}</small>
            </span>
          </summary>
          <ul>{rows(lib.fileTree(mine.map(function (f) { return f.path; }), dir))}</ul>
        </details>
      </li>;
    });
    // (A file in none of them, if there were one, is listed after them.)
    var loose = all.filter(function (f) { return !f.repo || !known[f.repo]; });
    return [shown, rows(lib.fileTree(loose.map(function (f) { return f.path; })))];
  };
  return <details class="diffnote-side" open>
    <summary>{lib.m('ui.tree.files_summary')}{files.length > 0 && <>{' '}<span class="diffnote-badge diffnote-badge--viewed" data-diffnote-viewed-count title={lib.m('ui.tree.viewed_count_title')}><Icon name="check" />{' '}{files.filter(function (f) { return isViewed(f, marks, revId); }).length}/{files.length}</span></>}</summary>
    <nav class={'diffnote-filelist' + (actions ? ' diffnote-filelist--menu' : '')} onContextMenu={onMenu}><ul>{repos.length ? groups() : rows(tree)}</ul>
      {ignored.length > 0 && <details class="diffnote-filelist__ignored" data-diffnote-ignored>
        <summary title={lib.m('ui.tree.ignored_title')}>{lib.mf('ui.tree.ignored_summary', { n: String(ignored.length) })}</summary>
        <ul>{names(lib.fileTree(ignored))}</ul>
      </details>}
    </nav>
    {target && actions && <TreeMenu target={target} close={function () { setTarget(null); }}
      leaveOut={function () {
        return actions!.saveSettings({ ignore: lib.withIgnored(ctx.model.settings && ctx.model.settings.ignore, target!.path) });
      }} />}
  </details>;
}

// What a right click in the file list offers, where it was made: leaving the
// directory or the file out. Goes on Escape, a click elsewhere, or a scroll
// (which would leave it where the row no longer is).
function TreeMenu(props: { target: Target; close: () => void; leaveOut: () => Promise<{ ok: boolean; error?: string }> }) {
  var t = props.target;
  var box = useRef<HTMLDivElement | null>(null);
  var _e = useState('');
  var error = _e[0];
  var setError = _e[1];
  useEffect(function () {
    var away = function (e: MouseEvent) { if (box.current && !box.current.contains(e.target as Node)) props.close(); };
    var key = function (e: KeyboardEvent) { if (e.key === 'Escape') props.close(); };
    var gone = function (e: Event) { if (!(box.current && box.current.contains(e.target as Node))) props.close(); };
    document.addEventListener('mousedown', away);
    document.addEventListener('keydown', key);
    document.addEventListener('scroll', gone, true);
    window.addEventListener('blur', props.close);
    return function () {
      document.removeEventListener('mousedown', away);
      document.removeEventListener('keydown', key);
      document.removeEventListener('scroll', gone, true);
      window.removeEventListener('blur', props.close);
    };
  }, []);
  // Kept inside the window (a click near its bottom or right edge would put
  // the menu past it); and its first item takes the keys, as a menu opened
  // from the keyboard should.
  useEffect(function () {
    var el = box.current;
    if (!el) return;
    var r = el.getBoundingClientRect();
    el.style.left = Math.max(0, Math.min(t.x, window.innerWidth - r.width - 4)) + 'px';
    el.style.top = Math.max(0, Math.min(t.y, window.innerHeight - r.height - 4)) + 'px';
    var first = el.querySelector('button');
    if (first) first.focus();
  }, [t.path, t.dir, t.x, t.y]);
  var pattern = lib.ignoreLine(t.path);
  return <div class="diffnote-comment__panel diffnote-treemenu" role="menu" ref={box} data-diffnote-tree-menu={t.path}
    style={'left:' + t.x + 'px;top:' + t.y + 'px'}>
    <button type="button" role="menuitem" class="diffnote-comment__item" data-diffnote-tree-ignore={t.path} disabled={t.blocked}
      title={t.blocked ? lib.m('ui.file.menu_ignore_blocked') : lib.mf('ui.tree.ignore_title', { pattern: pattern })}
      onClick={function () {
        props.leaveOut().then(function (res) {
          if (res.ok) props.close();
          else setError(res.error || lib.m('ui.save_failed'));
        });
      }}>{lib.m(t.dir ? 'ui.tree.ignore_dir' : 'ui.tree.ignore_file')}</button>
    {error && <span class="diffnote-error" role="alert">{error}</span>}
  </div>;
}

export function ThreadList(props: ListProps) {
  var ctx = props.ctx;
  var repos = ctx.revision.repos || [];
  var links = useContext(LinksContext);
  var open = ctx.model.threads.filter(function (t) { return !t.resolved; }).length;
  return <details class="diffnote-side" open>
    <summary>{lib.m('ui.thread.summary')} <span class="diffnote-badge" title={lib.m('ui.thread.summary_title')}>{open} / {ctx.model.threads.length}</span></summary>
    <nav class="diffnote-threadlist"><ol>
      {ctx.order.map(function (id) {
        var t = ctx.byId[id];
        var p = ctx.placements[id];
        var color = p && p.kind === 'line' ? lib.color(p.color) : '#8b949e';
        // A review of several repositories: the thread's file's, in its color.
        var file = p && 'file' in p ? p.file : null;
        var inRepo = file ? lib.repoOf(repos, file) : null;
        var repo = inRepo ? repos.filter(function (r) { return r.path === inRepo; })[0] : null;
        return <li key={id} class={(t.resolved ? 'is-resolved' : '') + (repo ? ' has-repo' : '')} data-diffnote-thread-repo={repo ? repo.path : undefined}
          style={repo ? '--diffnote-repo-color:' + lib.repoColor(repo.color) : undefined}>
          <a href={'#r' + ctx.rev + '-thread-' + id} data-diffnote-jump={id} title={lib.location(p) || lib.m('ui.thread.jump_title_fallback')}
            onClick={function (e) { e.preventDefault(); links.go({ kind: 'thread', id: id }); }}>
            <span class="diffnote-thread__swatch" style={'background:' + color}></span><span class="diffnote-threadlist__where">{lib.shortLocation(p)}</span>{t.resolved && <span class="diffnote-threadlist__state">{lib.m('ui.thread.resolved')}</span>}<span class="diffnote-threadlist__preview">{lib.withShortcodes(EMOJI, lib.preview((t.comments.filter(function (c) { return !c.deleted; })[0] || t.comments[0]).doc)) || lib.m('ui.thread.deleted_preview')}</span>
          </a>
        </li>;
      })}
    </ol></nav>
  </details>;
}
