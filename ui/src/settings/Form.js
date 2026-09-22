// 設定: what is saved in the review itself.
import { useEffect, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.js';
import { html } from '../html.ts';

// 設定: the review's settings, as they are kept in the bundle. Changed here
// and saved together: nothing is kept until 保存.
export function SettingsFormPane(props) {
  var model = props.model;
  var settings = model.settings || {};
  var _t = useState(settings.title || '');
  var title = _t[0];
  var setTitle = _t[1];
  var _w = useState(!!settings.ignore_whitespace);
  var ignore = _w[0];
  var setIgnore = _w[1];
  var _l = useState(String(lib.bytesToMB(settings.attachment_limit)));
  var limit = _l[0];
  var setLimit = _l[1];
  var _b = useState(false);
  var busy = _b[0];
  var setBusy = _b[1];
  var _e = useState('');
  var error = _e[0];
  var setError = _e[1];
  var _s = useState(false);
  var saved = _s[0];
  var setSaved = _s[1];
  var first = useRef(null);
  useEffect(function () { if (first.current) first.current.focus(); }, []);
  // What is here is not what is kept.
  var dirty = title.trim() !== (settings.title || '').trim() || ignore !== !!settings.ignore_whitespace
    || lib.mbToBytes(limit) !== settings.attachment_limit;
  var touched = function (set) { return function (v) { set(v); setSaved(false); setError(''); }; };
  var submit = function (e) {
    e.preventDefault();
    if (busy) return;
    var bytes = lib.mbToBytes(limit);
    if (bytes == null) { setError(lib.m('ui.settings.attachment_limit_not_number')); return; }
    if (bytes < 1024 || bytes > 100 * 1024 * 1024) { setError(lib.m('ui.settings.attachment_limit_out_of_range')); return; }
    setBusy(true);
    setError('');
    props.save({ title: title, ignore_whitespace: ignore, attachment_limit: bytes }).then(function (res) {
      setBusy(false);
      if (res.ok) setSaved(true);
      else setError(res.error || lib.m('ui.save_failed'));
    });
  };
  return html`<form class="diffnote-settings__form" data-diffnote-settings-pane noValidate onSubmit=${submit}>
    <h2>${lib.m('ui.settings.form_heading')}</h2>
    <p class="diffnote-settings__note">${lib.m('ui.settings.form_note')}</p>
    <label class="diffnote-field">
      <span>${lib.m('ui.settings.title_label')}</span>
      <input ref=${first} type="text" maxlength="200" data-diffnote-setting-title value=${title} placeholder=${lib.m('ui.settings.title_placeholder')}
        onInput=${function (e) { touched(setTitle)(e.target.value); }} />
    </label>
    <label class="diffnote-field diffnote-field--check">
      <input type="checkbox" data-diffnote-setting-ignore checked=${ignore} onChange=${function (e) { touched(setIgnore)(e.target.checked); }} />
      <span>${lib.m('ui.settings.ignore_ws_label')}<small>${lib.m('ui.settings.ignore_ws_hint')}</small></span>
    </label>
    <label class="diffnote-field">
      <span>${lib.m('ui.settings.attach_limit_label')}</span>
      <span class="diffnote-field__unit"><input type="number" step="any" data-diffnote-setting-limit value=${limit}
        onInput=${function (e) { touched(setLimit)(e.target.value); }} /> MB</span>
    </label>
    ${error && html`<p class="diffnote-error" role="alert">${error}</p>`}
    <div class="diffnote-reply__buttons">
      <button type="submit" class="diffnote-button diffnote-button--primary" data-diffnote-settings-save disabled=${busy || !dirty}>${lib.m('ui.save_button')}</button>
      ${saved && html`<span class="diffnote-settings__saved" data-diffnote-settings-saved role="status">${lib.m('ui.settings.saved_notice')}</span>`}
      ${dirty && !saved && html`<span class="diffnote-settings__dirty" data-diffnote-settings-dirty>${lib.m('ui.settings.dirty_notice')}</span>`}
    </div>
  </form>`;
}
