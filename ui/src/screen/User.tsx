// ユーザー設定: what is saved for every review of this user.
import { useEffect, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import type { IgnorePreset, ViewModel } from '../model.ts';
import type { ChangeAnswer } from '../state/contexts.ts';

// ユーザー設定: this machine's user settings (`diffnote config`; today, just
// the author name) -- not part of the bundle (applies to every review from
// now on, not only this one).
export interface UserProps {
  model: ViewModel;
  /** The name this machine has saved, if any. */
  configured?: string;
  save(author: string): Promise<ChangeAnswer>;
  savePresets(presets: IgnorePreset[]): Promise<ChangeAnswer>;
}

export function UserSettingsPane(props: UserProps) {
  var model = props.model;
  var configured = (model.user_settings && model.user_settings.author) || '';
  var _a = useState<string>(configured);
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
  var first = useRef<HTMLInputElement | null>(null);
  useEffect(function () { if (first.current) first.current.focus(); }, []);
  var dirty = author.trim() !== configured.trim();
  var touched = function (v: string) { setAuthor(v); setSaved(false); setError(''); };
  var submit = function (e: Event) {
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
  return <div data-diffnote-screen-pane>
  <form class="diffnote-screen__form" noValidate onSubmit={submit}>
    <h2>{lib.m('ui.user_settings.heading')}</h2>
    <p class="diffnote-screen__note">{lib.m('ui.user_settings.note')}</p>
    <label class="diffnote-field">
      <span>{lib.m('ui.user_settings.author_label')}</span>
      <input ref={first} type="text" maxlength={100} data-diffnote-user-setting-author value={author} placeholder={lib.m('ui.user_settings.author_placeholder')}
        onInput={function (e) { touched(e.currentTarget.value); }} />
    </label>
    {error && <p class="diffnote-error" role="alert">{error}</p>}
    <div class="diffnote-reply__buttons">
      <button type="submit" class="diffnote-button diffnote-button--primary" data-diffnote-user-settings-save disabled={busy || !dirty}>{lib.m('ui.save_button')}</button>
      {saved && <span class="diffnote-screen__saved" data-diffnote-user-settings-saved role="status">{lib.m('ui.settings.saved_notice')}</span>}
      {dirty && !saved && <span class="diffnote-screen__dirty" data-diffnote-user-settings-dirty>{lib.m('ui.settings.dirty_notice')}</span>}
    </div>
  </form>
  <PresetsForm model={model} save={props.savePresets} />
  </div>;
}

// 表示しないファイルのプリセット: named lists any review's settings can take
// with one press. Edited here together and saved together.
function PresetsForm(props: { model: ViewModel; save(presets: IgnorePreset[]): Promise<ChangeAnswer> }) {
  var kept = (props.model.user_settings && props.model.user_settings.ignore_presets) || [];
  var _p = useState<IgnorePreset[]>(kept);
  var presets = _p[0];
  var setPresets = _p[1];
  var _b = useState(false);
  var busy = _b[0];
  var setBusy = _b[1];
  var _e = useState('');
  var error = _e[0];
  var setError = _e[1];
  var _s = useState(false);
  var saved = _s[0];
  var setSaved = _s[1];
  var dirty = JSON.stringify(presets.map(function (p) { return [p.name.trim(), p.patterns.trimEnd()]; }))
    !== JSON.stringify(kept.map(function (p) { return [p.name, p.patterns]; }));
  var change = function (i: number, part: Partial<IgnorePreset>) {
    setPresets(presets.map(function (p, j) { return j === i ? Object.assign({}, p, part) : p; }));
    setSaved(false);
    setError('');
  };
  var submit = function (e: Event) {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError('');
    props.save(presets).then(function (res) {
      setBusy(false);
      if (res.ok) {
        setSaved(true);
        var now = res.model && res.model.user_settings && res.model.user_settings.ignore_presets;
        if (now) setPresets(now);
      } else setError(res.error || lib.m('ui.save_failed'));
    });
  };
  return <form class="diffnote-screen__form diffnote-presets" data-diffnote-presets noValidate onSubmit={submit}>
    <h3>{lib.m('ui.presets.heading')}</h3>
    <p class="diffnote-screen__note">{lib.m('ui.presets.note')}</p>
    {presets.length === 0 && <p class="diffnote-screen__note" data-diffnote-presets-empty>{lib.m('ui.presets.empty')}</p>}
    {presets.map(function (p, i) {
      return <fieldset key={i} class="diffnote-presets__item" data-diffnote-preset={i}>
        <label class="diffnote-field">
          <span>{lib.m('ui.presets.name_label')}</span>
          <span class="diffnote-setup__add-row">
            <input type="text" maxlength={100} data-diffnote-preset-name value={p.name} placeholder={lib.m('ui.presets.name_placeholder')}
              onInput={function (e) { change(i, { name: e.currentTarget.value }); }} />
            <button type="button" class="diffnote-button" data-diffnote-preset-remove
              onClick={function () { setPresets(presets.filter(function (_, j) { return j !== i; })); setSaved(false); }}>{lib.m('ui.presets.remove_button')}</button>
          </span>
        </label>
        <label class="diffnote-field">
          <span>{lib.m('ui.presets.patterns_label')}</span>
          <textarea rows={4} spellcheck={false} class="diffnote-field__code" data-diffnote-preset-patterns value={p.patterns}
            placeholder={lib.m('ui.settings.ignore_files_placeholder')}
            onInput={function (e) { change(i, { patterns: e.currentTarget.value }); }} />
        </label>
      </fieldset>;
    })}
    <p><button type="button" class="diffnote-button" data-diffnote-preset-add
      onClick={function () { setPresets(presets.concat([{ name: '', patterns: '' }])); setSaved(false); }}>{lib.m('ui.presets.add_button')}</button></p>
    {error && <p class="diffnote-error" role="alert">{error}</p>}
    <div class="diffnote-reply__buttons">
      <button type="submit" class="diffnote-button diffnote-button--primary" data-diffnote-presets-save disabled={busy || !dirty}>{lib.m('ui.save_button')}</button>
      {saved && <span class="diffnote-screen__saved" data-diffnote-presets-saved role="status">{lib.m('ui.settings.saved_notice')}</span>}
      {dirty && !saved && <span class="diffnote-screen__dirty">{lib.m('ui.settings.dirty_notice')}</span>}
    </div>
  </form>;
}
