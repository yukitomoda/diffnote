// One revision: its diff, and everything beside it.
import { useContext, useEffect, useMemo } from 'preact/hooks';
import { lib } from './lib.ts';
import { UserChip } from './UserChip.tsx';
import { ViewMenu } from './ViewMenu.tsx';
import { File } from './diff/File.jsx';
import { Tree } from './nav/Tree.tsx';
import { FileList, ThreadList } from './nav/lists.tsx';
import { OpenedContext, ViewedContext } from './state/contexts.ts';
import { Card } from './thread/Card.jsx';
import { Composer } from './thread/Composer.jsx';

// One revision: the side lists and the files.
export function Revision(props) {
  var model = props.model;
  var rev = props.index;
  // (Compared with an earlier revision instead of the base: another view of it.)
  var revision = props.override || model.revisions[rev];
  var byId = useMemo(function () {
    var m = {};
    model.threads.forEach(function (t) { m[t.id] = t; });
    return m;
  }, [model]);
  // The threads in the order they were written (their ids sort by time).
  // (A thread that the revision's data doesn't know yet, being written while it is
  // looked at against another, waits for that data to come again.)
  var order = useMemo(function () {
    return model.threads.map(function (t) { return t.id; }).filter(function (id) { return !!revision.placements[id]; });
  }, [model, revision]);
  // The files opened to look at come after the diff's; one that a thread has
  // since brought in keeps the lines that were opened.
  var opened = useContext(OpenedContext);
  var files = useMemo(function () {
    var mine = (opened && opened.byRev[rev]) || [];
    var seen = {};
    var merged = revision.files.map(function (f) {
      var o = mine.filter(function (x) { return x.path === f.path; })[0];
      if (!o) return f;
      seen[f.path] = true;
      return Object.assign({}, f, { hunks: o.hunks, opened: true, next: o.next, total: o.total });
    });
    mine.forEach(function (o) {
      if (!seen[o.path]) merged.push({ path: o.path, old_path: null, status: 'context', hunks: o.hunks, opened: true, next: o.next, total: o.total });
    });
    return merged;
  }, [revision, opened && opened.byRev[rev]]);
  var ctx = {
    model: model, rev: rev, revision: revision, byId: byId, order: order,
    placements: revision.placements, hideResolved: props.hideResolved, layout: props.layout, ignoreSpace: props.ignoreSpace, compare: !!props.override,
  };
  var globals = order.filter(function (id) { return revision.placements[id].kind === 'global'; });
  var viewed = useContext(ViewedContext);
  var viewedPaths = {};
  files.forEach(function (f) { if (viewed && viewed.is(f)) viewedPaths[f.path] = true; });
  var listOrder = { diffFiles: revision.files, viewedPaths: viewedPaths, model: model, rev: rev, revision: Object.assign({}, revision, { files: files }), hideResolved: props.hideResolved, byId: byId, order: revision.order, placements: revision.placements };

  // The file list marks the files that are on screen.
  useEffect(function () {
    if (!('IntersectionObserver' in window)) return undefined;
    var links = {};
    document.querySelectorAll('#rev-' + rev + ' .diffnote-filelist a').forEach(function (a) {
      links[(a.getAttribute('href') || '').slice(1)] = a;
      // Marked from nothing but what is observed below: a file that has
      // just been marked as looked at is no longer in the page at all, and
      // would otherwise keep the mark it had when it went.
      a.classList.remove('is-visible');
    });
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (en) {
        var a = links[en.target.id];
        if (a) a.classList.toggle('is-visible', en.isIntersecting);
      });
    }, { rootMargin: '-48px 0px -55% 0px' });
    document.querySelectorAll('#rev-' + rev + ' .diffnote-file').forEach(function (f) { io.observe(f); });
    return function () { io.disconnect(); };
  }, [rev, Object.keys(viewedPaths).join('\n')]);

  return <section class="diffnote-revision is-current" id={'rev-' + rev} data-diffnote-revision={rev}>
    <h2 class="diffnote-revision__title">{revision.label}</h2>
    <aside class="diffnote-sidebar">
      <div class="diffnote-sidebar__lists">
        <FileList ctx={listOrder} />
        {model.threads.length > 0 && <ThreadList ctx={listOrder} />}
        {opened && <Tree rev={rev} />}
      </div>
      {props.author != null && props.onToggleUserSettings && <UserChip name={props.author} open={props.userSettingsOpen} onToggle={props.onToggleUserSettings} />}
    </aside>
    <div class="diffnote-viewbar">
      {props.compose && <div class="diffnote-add"><button type="button" class="diffnote-button" data-diffnote-add="global"
        onClick={function () { props.compose.openScope('global', rev); }}>{lib.m('ui.compose.global_button')}</button></div>}
      {props.override && <p class="diffnote-compare-note" data-diffnote-compare-note tabindex="0" title={props.overrideNote.tip} aria-label={props.overrideNote.short + '。' + props.overrideNote.tip}>{props.overrideNote.short}<span class="diffnote-compare-note__icon" aria-hidden="true">⚠</span></p>}
      <ViewMenu />
    </div>
    {(globals.length > 0 || props.compose) && <section class="diffnote-global-comments" data-diffnote-global>
      {props.compose && props.compose.scope && props.compose.scope.kind === 'global' && props.compose.scope.rev === rev && <div class="diffnote-compose-wrap"><Composer scope="global" where={lib.m('ui.compose.global_where')} request={{ scope: 'global', revision: rev }} /></div>}
      {globals.map(function (id) { return <Card key={id} rev={rev} thread={byId[id]} placement={revision.placements[id]} />; })}
    </section>}
    {files.map(function (f) { return <File key={rev + ':' + f.path} file={f} ctx={ctx} />; })}
  </section>;
}
