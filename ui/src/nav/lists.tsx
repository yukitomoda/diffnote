// The lists beside the diff: the files, and the threads.
import { useContext } from 'preact/hooks';
import { EMOJI } from '../emoji.ts';
import { lib } from '../lib.ts';
import { htmlId } from '../dom.ts';
import { LinksContext, ViewedContext } from '../state/contexts.ts';
import type { ListCtx } from '../state/contexts.ts';

interface ListProps {
  ctx: ListCtx;
}

export function FileList(props: ListProps) {
  var ctx = props.ctx;
  var viewed = useContext(ViewedContext);
  var links = useContext(LinksContext);
  // The files of the diff (not those opened to look at) are what is counted.
  var files = ctx.diffFiles;
  return <details class="diffnote-side" open>
    <summary>{lib.m('ui.tree.files_summary')}{viewed && files.length > 0 && <>{' '}<span class="diffnote-badge diffnote-badge--viewed" data-diffnote-viewed-count title={lib.m('ui.tree.viewed_count_title')}>✓ {files.filter(viewed.is).length}/{files.length}</span></>}</summary>
    <nav class="diffnote-filelist"><ul>
      {ctx.revision.files.map(function (f) {
        var done = !!(viewed && viewed.is(f));
        var ids = lib.threadsOfFile(ctx.order, ctx.placements, f.path);
        // The threads that are shown: resolved ones don't count while hidden.
        var n = ids.filter(function (id) {
          return !(ctx.hideResolved && ctx.byId[id].resolved);
        }).length;
        // What is left open in a file that was looked at.
        var open = ids.filter(function (id) { return !ctx.byId[id].resolved; }).length;
        return <li key={f.path} class={done ? 'is-viewed' : ''}>
          {viewed && <button type="button" class="diffnote-check" data-diffnote-check={f.path} aria-pressed={done}
            title={done ? lib.m('ui.file.unmark_viewed_title') : lib.m('ui.file.mark_viewed_title')} onClick={function () { viewed.toggle(f); }}>{done ? '✓' : ''}</button>}
          <a href={'#r' + ctx.rev + '-file-' + htmlId(f.path)} data-diffnote-file-link={f.path}
            onClick={function (e) {
              // A file that was looked at comes back; one that was folded opens; and it is marked.
              e.preventDefault();
              links.go({ kind: 'file', path: f.path });
            }}>{f.path}</a>
          {done
            ? open > 0 && <span class="diffnote-badge" data-diffnote-open-count title={lib.mf('ui.thread.open_count_title', { n: String(open) })}>{open}</span>
            : n > 0 && <>{' '}<span class="diffnote-badge">{n}</span></>}
        </li>;
      })}
    </ul></nav>
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
