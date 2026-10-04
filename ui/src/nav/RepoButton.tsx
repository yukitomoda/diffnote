// A review of several repositories: the button at the top that lists the
// revision's, each in its color, with what it is compared from and up to and
// how many of its files changed. A press on one goes to it: its group in the
// file list (opened), and its first file in the diff.
import { useEffect, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { htmlId } from '../dom.ts';
import type { RevisionData } from '../model.ts';

export function RepoButton(props: { rev: number; revision: RevisionData }) {
  var _o = useState(false);
  var open = _o[0];
  var setOpen = _o[1];
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
  var repos = props.revision.repos || [];
  if (!repos.length) return null;
  var files = props.revision.files;
  var go = function (path: string) {
    setOpen(false);
    var group = document.querySelector('#rev-' + props.rev + ' [data-diffnote-repo-group="' + CSS.escape(path) + '"]');
    if (group) {
      var folded = group.querySelector('details');
      if (folded) folded.open = true;
      group.scrollIntoView({ block: 'nearest' });
    }
    // (A file marked as looked at is not in the page: the first one that is.)
    for (var f of files) {
      var shown = f.repo === path && document.getElementById('r' + props.rev + '-file-' + htmlId(f.path));
      if (shown) { shown.scrollIntoView({ block: 'start' }); break; }
    }
  };
  return <span class="diffnote-repolist" ref={box}>
    <button type="button" class="diffnote-repolist__button" data-diffnote-repos-button aria-haspopup="true" aria-expanded={open}
      title={lib.m('ui.repolist.button_title')} onClick={function () { setOpen(!open); }}>
      {repos.map(function (r) {
        return <span key={r.path} class="diffnote-repo-dot" aria-hidden="true" style={'--diffnote-repo-color:' + lib.repoColor(r.color)}></span>;
      })}
      {lib.mf('ui.repolist.button', { n: String(repos.length) })}
    </button>
    <div class="diffnote-viewmenu__panel diffnote-repolist__panel" role="menu" hidden={!open} data-diffnote-repos-panel>
      {repos.map(function (r) {
        var changed = files.filter(function (f) { return f.repo === r.path && f.status !== 'context'; }).length;
        return <button type="button" role="menuitem" key={r.path} class="diffnote-repolist__item" data-diffnote-repo-jump={r.path}
          style={'--diffnote-repo-color:' + lib.repoColor(r.color)} title={lib.mf('ui.repolist.jump_title', { path: r.path })}
          onClick={function () { go(r.path); }}>
          <span class="diffnote-repo-dot" aria-hidden="true"></span>
          <span class="diffnote-repolist__name">
            <code>{r.path}</code>
            <small>{r.target} {r.base}→{r.head}</small>
          </span>
          <small class="diffnote-repolist__count">{changed > 0 ? lib.mf('ui.repolist.files', { n: String(changed) }) : lib.m('ui.repolist.no_files')}</small>
        </button>;
      })}
    </div>
  </span>;
}
