// ユーザー設定: what is saved for every review of this user.
import { useEffect, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { html } from '../html.ts';

// ユーザー設定: this machine's user settings (`diffnote config`; today, just
// the author name) -- not part of the bundle (applies to every review from
// now on, not only this one).
export function UserSettingsPane(props) {
  var model = props.model;
  var configured = (model.user_settings && model.user_settings.author) || '';
  var _a = useState(configured);
  var author = _a[0];
  var setAuthor = _a[1];
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
  var dirty = author.trim() !== configured.trim();
  var touched = function (v) { setAuthor(v); setSaved(false); setError(''); };
  var submit = function (e) {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError('');
    props.save(author).then(function (res) {
      setBusy(false);
      if (res.ok) setSaved(true);
      else setError(res.error || lib.m('ui.save_failed'));
    });
  };
  return html`<form class="diffnote-settings__form" data-diffnote-settings-pane noValidate onSubmit=${submit}>
    <h2>${lib.m('ui.user_settings.heading')}</h2>
    <p class="diffnote-settings__note">${lib.m('ui.user_settings.note')}</p>
    <label class="diffnote-field">
      <span>${lib.m('ui.user_settings.author_label')}</span>
      <input ref=${first} type="text" maxlength="100" data-diffnote-user-setting-author value=${author} placeholder=${lib.m('ui.user_settings.author_placeholder')}
        onInput=${function (e) { touched(e.target.value); }} />
    </label>
    ${error && html`<p class="diffnote-error" role="alert">${error}</p>`}
    <div class="diffnote-reply__buttons">
      <button type="submit" class="diffnote-button diffnote-button--primary" data-diffnote-user-settings-save disabled=${busy || !dirty}>${lib.m('ui.save_button')}</button>
      ${saved && html`<span class="diffnote-settings__saved" data-diffnote-user-settings-saved role="status">${lib.m('ui.settings.saved_notice')}</span>`}
      ${dirty && !saved && html`<span class="diffnote-settings__dirty" data-diffnote-user-settings-dirty>${lib.m('ui.settings.dirty_notice')}</span>`}
    </div>
  </form>`;
}
