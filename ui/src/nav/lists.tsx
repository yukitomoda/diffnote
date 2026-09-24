// The lists beside the diff: the files, and the threads.
import { useContext } from 'preact/hooks';
import { EMOJI } from '../emoji.ts';
import { lib } from '../lib.ts';
import { htmlId } from '../dom.ts';
import { useStore } from '@nanostores/preact';
import { LinksContext } from '../state/contexts.ts';
import { isViewed, seen, toggleViewed } from '../state/viewed.ts';
import type { ListCtx } from '../state/contexts.ts';
import type { FileTreeNode } from '../lib.ts';
import { Icon } from '../icon.tsx';

interface ListProps {
  ctx: ListCtx;
}

// The files, as a tree of their directories (see `lib.fileTree`). Each file
// says how many threads are on it, and, at the right, whether it was looked at.
export function FileList(props: ListProps) {
  var ctx = props.ctx;
  var marks = useStore(seen);
  var links = useContext(LinksContext);
  // The files of the diff (not those opened to look at) are what is counted.
  var files = ctx.diffFiles;
  var byPath: Record<string, (typeof ctx.revision.files)[number]> = {};
  ctx.revision.files.forEach(function (f) { byPath[f.path] = f; });
  var tree = lib.fileTree(ctx.revision.files.map(function (f) { return f.path; }));
  var file = function (node: FileTreeNode) {
    var f = byPath[node.path!];
    var done = isViewed(f, marks);
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
        }}>{node.label}</a>
      {done
        ? open > 0 && <span class="diffnote-badge" data-diffnote-open-count title={lib.mf('ui.thread.open_count_title', { n: String(open) })}>{open}</span>
        : n > 0 && <span class="diffnote-badge">{n}</span>}
      <button type="button" class="diffnote-check" data-diffnote-check={f.path} aria-pressed={done}
        title={done ? lib.m('ui.file.unmark_viewed_title') : lib.m('ui.file.mark_viewed_title')} onClick={function () { toggleViewed(f); }}><span class={'diffnote-tick' + (done ? ' is-on' : '')}><Icon name="check" /></span></button>
    </li>;
  };
  var rows = function (nodes: FileTreeNode[]): preact.ComponentChildren {
    return nodes.map(function (node) {
      if (node.path != null) return file(node);
      return <li key={'dir:' + node.label} class="diffnote-filelist__dir">
        <span class="diffnote-filelist__dirname">{node.label}</span>
        <ul>{rows(node.children)}</ul>
      </li>;
    });
  };
  return <details class="diffnote-side" open>
    <summary>{lib.m('ui.tree.files_summary')}{files.length > 0 && <>{' '}<span class="diffnote-badge diffnote-badge--viewed" data-diffnote-viewed-count title={lib.m('ui.tree.viewed_count_title')}><Icon name="check" />{' '}{files.filter(function (f) { return isViewed(f, marks); }).length}/{files.length}</span></>}</summary>
    <nav class="diffnote-filelist"><ul>{rows(tree)}</ul></nav>
  </details>;
}

export function ThreadList(props: ListProps) {
  var ctx = props.ctx;
  var links = useContext(LinksContext);
  var open = ctx.model.threads.filter(function (t) { return !t.resolved; }).length;
  return <details class="diffnote-side" open>
    <summary>{lib.m('ui.thread.summary')} <span class="diffnote-badge" title={lib.m('ui.thread.summary_title')}>{open} / {ctx.model.threads.length}</span></summary>
    <nav class="diffnote-threadlist"><ol>
      {ctx.order.map(function (id) {
        var t = ctx.byId[id];
        var p = ctx.placements[id];
        var color = p && p.kind === 'line' ? lib.color(p.color) : '#8b949e';
        return <li key={id} class={t.resolved ? 'is-resolved' : ''}>
          <a href={'#r' + ctx.rev + '-thread-' + id} data-diffnote-jump={id} title={lib.location(p) || lib.m('ui.thread.jump_title_fallback')}
            onClick={function (e) { e.preventDefault(); links.go({ kind: 'thread', id: id }); }}>
            <span class="diffnote-thread__swatch" style={'background:' + color}></span><span class="diffnote-threadlist__where">{lib.shortLocation(p)}</span>{t.resolved && <span class="diffnote-threadlist__state">{lib.m('ui.thread.resolved')}</span>}<span class="diffnote-threadlist__preview">{lib.withShortcodes(EMOJI, lib.preview((t.comments.filter(function (c) { return !c.deleted; })[0] || t.comments[0]).doc)) || lib.m('ui.thread.deleted_preview')}</span>
          </a>
        </li>;
      })}
    </ol></nav>
  </details>;
}
