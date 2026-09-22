// The 表示 menu over a diff: how it is shown.
import { useEffect, useRef, useState } from 'preact/hooks';
import { useStore } from '@nanostores/preact';
import { lib } from './lib.ts';
import { Icon } from './icon.tsx';
import {
  hideResolved,
  ignoreWhitespace,
  layout as shownLayout,
  setHideResolved,
  setIgnoreWhitespace,
  setLayout,
  wide as haveRoom,
} from './state/view.ts';

interface ViewMenuProps {
  /** How many threads are resolved, and whether the page can change them. */
  resolved: number;
  interactive: boolean;
}

// 「表示」: how the diff is shown, in a menu at the top right of the diff.
export function ViewMenu(props: ViewMenuProps) {
  var wide = useStore(haveRoom);
  var layout = useStore(shownLayout);
  var hide = useStore(hideResolved);
  var ignoreSpace = useStore(ignoreWhitespace);
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
      onClick={function () { setOpen(!open); }}><Icon name="view" />{' '}{lib.m('ui.viewmenu.button')}<Icon name="down" /></button>
    <div class="diffnote-viewmenu__panel" hidden={!open} data-diffnote-view-panel>
      {wide && <div class="diffnote-layout" role="group" aria-label={lib.m('ui.viewmenu.layout_label')}>
        <p class="diffnote-viewmenu__head">{lib.m('ui.viewmenu.layout_label')}</p>
        {(['unified', 'split'] as const).map(function (kind) {
          var label = kind === 'unified' ? lib.m('ui.viewmenu.layout_unified') : lib.m('ui.viewmenu.layout_split');
          return <button type="button" key={kind} class={'diffnote-viewmenu__item diffnote-layout__button' + (kind === layout ? ' is-current' : '')} data-diffnote-layout={kind}
            onClick={function () { setLayout(kind); }}><span class="diffnote-viewmenu__mark">{kind === layout && <Icon name="check" />}</span>{label}</button>;
        })}
        <hr />
      </div>}
      <label class={'diffnote-viewmenu__item' + (ignoreSpace ? ' is-current' : '')} title={lib.m('ui.viewmenu.ignore_space_title')}>
        <input type="checkbox" data-diffnote-ignore-space checked={ignoreSpace} onChange={function (e) { setIgnoreWhitespace(e.currentTarget.checked); }} /><span class="diffnote-viewmenu__mark">{ignoreSpace && <Icon name="check" />}</span>{lib.m('ui.viewmenu.ignore_space_label')}
      </label>
      {(props.resolved > 0 || props.interactive) && <label class={'diffnote-viewmenu__item' + (hide ? ' is-current' : '')}>
        <input type="checkbox" data-diffnote-hide-resolved checked={hide} onChange={function (e) { setHideResolved(e.currentTarget.checked); }} /><span class="diffnote-viewmenu__mark">{hide && <Icon name="check" />}</span>{lib.m('ui.viewmenu.hide_resolved_label')}<span class="diffnote-toggle__count" data-diffnote-resolved-count>{'(' + props.resolved + ')'}</span>
      </label>}
    </div>
  </div>;
}
