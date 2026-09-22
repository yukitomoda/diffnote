// The ⚙ menu over a diff: how it is shown.
import { useContext, useEffect, useRef, useState } from 'preact/hooks';
import { lib } from './lib.ts';
import { ViewContext } from './state/contexts.ts';

// 「表示」: how the diff is shown, in a menu at the top right of the diff.
export function ViewMenu() {
  var v = useContext(ViewContext);
  var _o = useState(false);
  var open = _o[0];
  var setOpen = _o[1];
  var box = useRef<HTMLDivElement | null>(null);
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
  // (The panel is in the page while it is closed, only not shown.)
  return <div class="diffnote-viewmenu" ref={box}>
    <button type="button" class="diffnote-button" data-diffnote-view-menu aria-haspopup="true" aria-expanded={open}
      onClick={function () { setOpen(!open); }}>{lib.m('ui.viewmenu.button')}</button>
    <div class="diffnote-viewmenu__panel" hidden={!open} data-diffnote-view-panel>
      {v.wide && <div class="diffnote-layout" role="group" aria-label={lib.m('ui.viewmenu.layout_label')}>
        <p class="diffnote-viewmenu__head">{lib.m('ui.viewmenu.layout_label')}</p>
        {(['unified', 'split'] as const).map(function (kind) {
          var label = kind === 'unified' ? lib.m('ui.viewmenu.layout_unified') : lib.m('ui.viewmenu.layout_split');
          return <button type="button" key={kind} class={'diffnote-viewmenu__item diffnote-layout__button' + (kind === v.layout ? ' is-current' : '')} data-diffnote-layout={kind}
            onClick={function () { v.setLayout(kind); }}>{label}</button>;
        })}
        <hr />
      </div>}
      <label class={'diffnote-viewmenu__item' + (v.ignoreSpace ? ' is-current' : '')} title={lib.m('ui.viewmenu.ignore_space_title')}>
        <input type="checkbox" data-diffnote-ignore-space checked={v.ignoreSpace} onChange={function (e) { v.toggleSpace(e.currentTarget.checked); }} />{lib.m('ui.viewmenu.ignore_space_label')}
      </label>
      {(v.resolved > 0 || v.interactive) && <label class={'diffnote-viewmenu__item' + (v.hide ? ' is-current' : '')}>
        <input type="checkbox" data-diffnote-hide-resolved checked={v.hide} onChange={function (e) { v.setHide(e.currentTarget.checked); }} />{lib.m('ui.viewmenu.hide_resolved_label')}<span class="diffnote-toggle__count" data-diffnote-resolved-count>{'(' + v.resolved + ')'}</span>
      </label>}
    </div>
  </div>;
}
