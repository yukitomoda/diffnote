// Stopping the server, and what is said afterwards.
import { render } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.js';
import { transport } from '../transport.js';
import { html } from '../html.js';
import { kept } from '../state/kept.js';

// 「終了」: the way to finish, big; and, behind the arrow, the way not to
// keep what was done in this session (asked again before it is done).
export function QuitButton() {
  var _o = useState(false);
  var open = _o[0];
  var setOpen = _o[1];
  var _s = useState(false);
  var sure = _s[0];
  var setSure = _s[1];
  var _e = useState(null);
  var error = _e[0];
  var setError = _e[1];
  var box = useRef(null);
  useEffect(function () {
    if (!open) return undefined;
    var away = function (e) { if (box.current && !box.current.contains(e.target)) { setOpen(false); setSure(false); } };
    var key = function (e) { if (e.key === 'Escape') { setOpen(false); setSure(false); } };
    document.addEventListener('mousedown', away);
    document.addEventListener('keydown', key);
    return function () {
      document.removeEventListener('mousedown', away);
      document.removeEventListener('keydown', key);
    };
  }, [open]);
  var quit = function (discard) {
    transport.post('/api/shutdown', discard ? { discard: true } : undefined).then(function (res) {
      if (res.ok) stopped(res.summary);
      else setError(res.error || lib.m('ui.quit.shutdown_failed'));
    });
  };
  return html`<span class="diffnote-quit" ref=${box}>
    <button type="button" class="diffnote-quit__main" data-diffnote-shutdown title=${lib.m('ui.quit.main_title')}
      onClick=${function () { quit(false); }}>${lib.m('ui.quit.main_button')}</button><button type="button" class="diffnote-quit__more" data-diffnote-quit-more aria-label=${lib.m('ui.quit.more_label')} aria-expanded=${open}
      onClick=${function () { setOpen(!open); setSure(false); }}>▾</button>
    ${open && html`<div class="diffnote-quit__menu" data-diffnote-quit-menu>
      ${!sure
        ? html`<button type="button" class="diffnote-quit__item" data-diffnote-discard onClick=${function () { setSure(true); }}>${lib.m('ui.quit.discard_button')}</button>`
        : html`<p>${lib.m('ui.quit.confirm_note')}</p>
          <button type="button" class="diffnote-quit__danger" data-diffnote-discard-confirm onClick=${function () { quit(true); }}>${lib.m('ui.quit.discard_confirm_button')}</button>
          <button type="button" class="diffnote-quit__cancel" onClick=${function () { setSure(false); setOpen(false); }}>${lib.m('ui.confirm_cancel')}</button>`}
      ${error && html`<p class="diffnote-error">${error}</p>`}
    </div>`}
  </span>`;
}

// What the tab shows once the server has stopped.
function stopped(summary) {
  render(null, document.getElementById('app'));
  var make = function (tag, cls, text) {
    var e = document.createElement(tag);
    e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };
  var card = make('div', 'diffnote-farewell__card');
  var discarded = !!(summary && summary.discarded);
  card.appendChild(make('div', 'diffnote-farewell__mark', discarded ? '↩' : '✓'));
  card.appendChild(make('h1', 'diffnote-farewell__title', discarded ? lib.m('ui.farewell.discarded_title') : lib.m('ui.farewell.done_title')));
  if (discarded) {
    var kept = make('dl', 'diffnote-farewell__list');
    kept.appendChild(make('dt', '', lib.m('ui.farewell.changes_label')));
    kept.appendChild(make('dd', '', lib.m('ui.farewell.discarded_value')));
    kept.appendChild(make('dt', '', lib.m('ui.farewell.path_label')));
    kept.appendChild(make('dd', '', summary.path + (summary.removed ? lib.m('ui.farewell.removed_suffix') : lib.m('ui.farewell.kept_suffix'))));
    card.appendChild(kept);
  } else if (summary) {
    var list = make('dl', 'diffnote-farewell__list');
    var row = function (label, value) {
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
