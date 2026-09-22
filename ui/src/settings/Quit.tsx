// Stopping the server, and what is said afterwards.
import { render } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { server } from '../transport.ts';

/** What the server says as it stops. */
interface Farewell {
  discarded?: boolean;
  /** Where the review was saved, and whether it was taken away instead. */
  path?: string;
  removed?: boolean;
  /** What was done to it, said in a line, and the count behind it. */
  changes?: string;
  threads?: number;
  comments?: number;
}

// 「終了」: the way to finish, big; and, behind the arrow, the way not to
// keep what was done in this session (asked again before it is done).
export function QuitButton() {
  var _o = useState(false);
  var open = _o[0];
  var setOpen = _o[1];
  var _s = useState(false);
  var sure = _s[0];
  var setSure = _s[1];
  var _e = useState<string | null>(null);
  var error = _e[0];
  var setError = _e[1];
  var box = useRef<HTMLElement | null>(null);
  useEffect(function () {
    if (!open) return undefined;
    var away = function (e: MouseEvent) { if (box.current && !box.current.contains(e.target as Node)) { setOpen(false); setSure(false); } };
    var key = function (e: KeyboardEvent) { if (e.key === 'Escape') { setOpen(false); setSure(false); } };
    document.addEventListener('mousedown', away);
    document.addEventListener('keydown', key);
    return function () {
      document.removeEventListener('mousedown', away);
      document.removeEventListener('keydown', key);
    };
  }, [open]);
  var quit = function (discard: boolean) {
    server().post<{ summary: Farewell }>('/api/shutdown', discard ? { discard: true } : undefined).then(function (res) {
      if (res.ok) stopped(res.summary);
      else setError(res.error || lib.m('ui.quit.shutdown_failed'));
    });
  };
  return <span class="diffnote-quit" ref={box}>
    <button type="button" class="diffnote-quit__main" data-diffnote-shutdown title={lib.m('ui.quit.main_title')}
      onClick={function () { quit(false); }}>{lib.m('ui.quit.main_button')}</button><button type="button" class="diffnote-quit__more" data-diffnote-quit-more aria-label={lib.m('ui.quit.more_label')} aria-expanded={open}
      onClick={function () { setOpen(!open); setSure(false); }}>▾</button>
    {open && <div class="diffnote-quit__menu" data-diffnote-quit-menu>
      {!sure
        ? <button type="button" class="diffnote-quit__item" data-diffnote-discard onClick={function () { setSure(true); }}>{lib.m('ui.quit.discard_button')}</button>
        : <><p>{lib.m('ui.quit.confirm_note')}</p><button type="button" class="diffnote-quit__danger" data-diffnote-discard-confirm onClick={function () { quit(true); }}>{lib.m('ui.quit.discard_confirm_button')}</button><button type="button" class="diffnote-quit__cancel" onClick={function () { setSure(false); setOpen(false); }}>{lib.m('ui.confirm_cancel')}</button></>}
      {error && <p class="diffnote-error">{error}</p>}
    </div>}
  </span>;
}

// What the tab shows once the server has stopped.
function stopped(summary: Farewell | undefined) {
  render(null, document.getElementById('app') as HTMLElement);
  var make = function (tag: string, cls: string, text?: string) {
    var e = document.createElement(tag);
    e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };
  var card = make('div', 'diffnote-farewell__card');
  var discarded = !!(summary && summary.discarded);
  card.appendChild(make('div', 'diffnote-farewell__mark', discarded ? '↩' : '✓'));
  card.appendChild(make('h1', 'diffnote-farewell__title', discarded ? lib.m('ui.farewell.discarded_title') : lib.m('ui.farewell.done_title')));
  if (discarded && summary) {
    var discardedList = make('dl', 'diffnote-farewell__list');
    discardedList.appendChild(make('dt', '', lib.m('ui.farewell.changes_label')));
    discardedList.appendChild(make('dd', '', lib.m('ui.farewell.discarded_value')));
    discardedList.appendChild(make('dt', '', lib.m('ui.farewell.path_label')));
    discardedList.appendChild(make('dd', '', summary.path + (summary.removed ? lib.m('ui.farewell.removed_suffix') : lib.m('ui.farewell.kept_suffix'))));
    card.appendChild(discardedList);
  } else if (summary) {
    var list = make('dl', 'diffnote-farewell__list');
    var row = function (label: string, value?: string) {
      list.appendChild(make('dt', '', label));
      list.appendChild(make('dd', '', value));
    };
    row(lib.m('ui.farewell.changes_label'), summary.changes);
    row(lib.m('ui.farewell.path_label'), summary.path);
    if (summary.threads != null) {
      row(lib.m('ui.farewell.summary_label'), lib.mf('ui.farewell.summary_value', { threads: String(summary.threads), comments: String(summary.comments) }));
    }
    card.appendChild(list);
  }
  card.appendChild(make('p', 'diffnote-farewell__note', lib.m('ui.farewell.note')));
  var page = make('div', 'diffnote-farewell');
  page.appendChild(card);
  document.body.replaceChildren(page);
}
