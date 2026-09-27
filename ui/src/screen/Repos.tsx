// リポジトリ: which repositories a review of several is of, from the next
// revision on. One can be taken out (what was recorded of it stays), and
// one added, from a commit chosen as on the first screen.
import { useEffect, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { server } from '../transport.ts';
import type { SetupPreview, SetupRepo, SetupSpan, ViewModel } from '../model.ts';
import type { ChangeAnswer } from '../state/contexts.ts';
import { BasePicker, RepoHead, TargetField } from './Setup.tsx';
import { baseOf, rowOf, targetSettled } from './setup.ts';
import type { RepoRow } from './setup.ts';

export interface ReposProps {
  model: ViewModel;
  add(path: string, base: string, target?: string): Promise<ChangeAnswer>;
  remove(path: string): Promise<ChangeAnswer>;
  setTarget(path: string, target: string): Promise<ChangeAnswer>;
}

export function ReposPane(props: ReposProps) {
  var repos = (props.model.workspace && props.model.workspace.repos) || [];
  // The one being taken out (asked once more before it is), and what went wrong.
  var _r = useState<string | null>(null);
  var removing = _r[0];
  var setRemoving = _r[1];
  var _e = useState('');
  var error = _e[0];
  var setError = _e[1];
  // The one whose target is being changed, and what is typed.
  var _t = useState<{ path: string; target: string } | null>(null);
  var editing = _t[0];
  var setEditing = _t[1];
  var saveTarget = function () {
    if (!editing) return;
    setBusy(true);
    setError('');
    props.setTarget(editing.path, editing.target).then(function (res) {
      setBusy(false);
      if (res.ok) setEditing(null);
      else setError(res.error || lib.m('ui.save_failed'));
    });
  };
  var _b = useState(false);
  var busy = _b[0];
  var setBusy = _b[1];
  // Repositories under the directory the review isn't of: offered to add.
  var _f = useState<string[] | null>(null);
  var found = _f[0];
  var setFound = _f[1];
  useEffect(function () {
    server().get<{ found: string[] }>('/api/repos/found').then(function (res) { setFound(res.ok ? res.found : []); });
  }, [repos.length]);
  // The one being added: looked up by its path, then its base chosen.
  var _p = useState('');
  var path = _p[0];
  var setPath = _p[1];
  var _a = useState<RepoRow | null>(null);
  var adding = _a[0];
  var setAdding = _a[1];
  var _ae = useState('');
  var addError = _ae[0];
  var setAddError = _ae[1];
  // A part of the row being added, merged into it as it is then.
  var patchAdding = function (part: Partial<RepoRow>) {
    setAdding(function (cur) { return cur ? Object.assign({}, cur, part) : cur; });
  };
  var lookUp = function (at: string) {
    at = at.trim();
    if (!at) return;
    setAddError('');
    setAdding(null);
    server().get<{ repo: SetupRepo }>('/api/repos/at?path=' + encodeURIComponent(at)).then(function (res) {
      if (res.ok) setAdding(rowOf(res.repo));
      else setAddError(res.error);
    });
  };
  var preview = function (repoPath: string, rev: string) {
    return server().get<{ preview: SetupPreview }>('/api/repos/preview?path=' + encodeURIComponent(repoPath) + '&rev=' + encodeURIComponent(rev));
  };
  var span = function (repoPath: string, base: string, target: string) {
    return server().get<{ span: SetupSpan }>('/api/repos/preview?path=' + encodeURIComponent(repoPath) + '&rev=' + encodeURIComponent(target) + '&from=' + encodeURIComponent(base));
  };
  var remove = function (at: string) {
    setBusy(true);
    setError('');
    props.remove(at).then(function (res) {
      setBusy(false);
      setRemoving(null);
      if (!res.ok) setError(res.error || lib.m('ui.save_failed'));
    });
  };
  var add = function () {
    if (!adding) return;
    var base = baseOf(adding);
    if (!base || (adding.base === 'manual' && !adding.preview) || !targetSettled(adding)) return;
    setBusy(true);
    setAddError('');
    var target = adding.target.trim();
    props.add(adding.info.path, base, target === 'HEAD' ? undefined : target).then(function (res) {
      setBusy(false);
      if (res.ok) { setAdding(null); setPath(''); }
      else setAddError(res.error || lib.m('ui.save_failed'));
    });
  };
  var canAdd = !!adding && !!baseOf(adding) && !(adding.base === 'manual' && (adding.checking || !adding.preview)) && targetSettled(adding);
  return <div data-diffnote-repos-pane>
    <h2>{lib.m('ui.repos.heading')}</h2>
    <p class="diffnote-screen__note">{lib.m('ui.repos.note')}</p>
    <ul class="diffnote-repos">
      {repos.map(function (r) {
        return <li key={r.path} class="diffnote-repos__item" data-diffnote-repo={r.path}>
          <code class="diffnote-repos__path">{r.path}</code>
          <span class="diffnote-repos__base">{lib.mf('ui.repos.base', { id: r.base })}</span>
          {editing && editing.path === r.path
            ? <span class="diffnote-repos__target-edit" data-diffnote-repo-target-edit>
                <span class="diffnote-repos__target">{lib.m('ui.setup.target_label')}:</span>
                <input type="text" data-diffnote-repo-target-rev value={editing.target} onInput={function (e) { setEditing({ path: r.path, target: e.currentTarget.value }); }}
                  onKeyDown={function (e) { if (e.key === 'Enter') { e.preventDefault(); saveTarget(); } }} />
                <button type="button" class="diffnote-button diffnote-button--primary" data-diffnote-repo-target-save disabled={busy || !editing.target.trim()} onClick={saveTarget}>{lib.m('ui.repos.target_save')}</button>
                <button type="button" class="diffnote-button" onClick={function () { setEditing(null); setError(''); }}>{lib.m('ui.confirm_cancel')}</button>
              </span>
            : <span class="diffnote-repos__target" data-diffnote-repo-target={r.target}>{lib.mf('ui.repos.target', { target: r.target })}{' '}
                <button type="button" class="diffnote-mini" data-diffnote-repo-target-change onClick={function () { setEditing({ path: r.path, target: r.target }); setError(''); }}>{lib.m('ui.repos.target_change')}</button></span>}
          {!r.present && <span class="diffnote-repos__away" data-diffnote-repo-away>{lib.m('ui.repos.away')}</span>}
          {removing === r.path
            ? <span class="diffnote-repos__confirm">
                <span>{lib.m('ui.repos.remove_confirm')}</span>
                <button type="button" class="diffnote-button diffnote-button--danger" data-diffnote-repo-remove-confirm disabled={busy} onClick={function () { remove(r.path); }}>{lib.m('ui.repos.remove_button')}</button>
                <button type="button" class="diffnote-button" onClick={function () { setRemoving(null); }}>{lib.m('ui.confirm_cancel')}</button>
              </span>
            : <button type="button" class="diffnote-button diffnote-repos__remove" data-diffnote-repo-remove disabled={busy || repos.length < 2}
                title={repos.length < 2 ? lib.m('ui.repos.last_title') : undefined}
                onClick={function () { setRemoving(r.path); setError(''); }}>{lib.m('ui.repos.remove_button')}</button>}
        </li>;
      })}
    </ul>
    {error && <p class="diffnote-error" role="alert">{error}</p>}

    <h3>{lib.m('ui.repos.add_heading')}</h3>
    {/* The one being added comes first, above the list it was picked from
        (which can be long): the form is then where the eye is. */}
    {adding && <div class="diffnote-repos__adding" data-diffnote-repo-adding={adding.info.path} ref={function (el) { if (el && !el.dataset.shown) { el.dataset.shown = '1'; el.scrollIntoView({ block: 'nearest' }); } }}>
      <p><code>{adding.info.path}</code></p>
      <RepoHead info={adding.info} />
      <p class="diffnote-screen__note">{lib.m('ui.setup.base_label')}</p>
      <BasePicker row={adding} id="add" preview={preview} span={span} patch={patchAdding} />
      <TargetField row={adding} id="add" preview={preview} span={span} patch={patchAdding} />
      <div class="diffnote-reply__buttons">
        <button type="button" class="diffnote-button diffnote-button--primary" data-diffnote-repo-add disabled={busy || !canAdd} onClick={add}>{lib.m('ui.repos.add_button')}</button>
        <button type="button" class="diffnote-button" onClick={function () { setAdding(null); }}>{lib.m('ui.confirm_cancel')}</button>
      </div>
    </div>}
    {addError && <p class="diffnote-error" data-diffnote-repo-add-error role="alert">{addError}</p>}
    {found && found.length > 0 && <ul class="diffnote-repos diffnote-repos--found" data-diffnote-repos-found>
      {found.map(function (p) {
        return <li key={p} class="diffnote-repos__item">
          <code class="diffnote-repos__path">{p}</code>
          <button type="button" class="diffnote-button" data-diffnote-repo-pick={p} onClick={function () { setPath(p); lookUp(p); }}>{lib.m('ui.repos.pick_button')}</button>
        </li>;
      })}
    </ul>}
    <label class="diffnote-field">
      <span>{lib.m('ui.repos.path_label')}</span>
      <span class="diffnote-setup__add-row">
        <input type="text" data-diffnote-repo-path value={path} placeholder={lib.m('ui.setup.add_path_placeholder')}
          onInput={function (e) { setPath(e.currentTarget.value); setAddError(''); }}
          onKeyDown={function (e) { if (e.key === 'Enter') { e.preventDefault(); lookUp(path); } }} />
        <button type="button" class="diffnote-button" data-diffnote-repo-look disabled={!path.trim()} onClick={function () { lookUp(path); }}>{lib.m('ui.repos.look_button')}</button>
      </span>
    </label>
  </div>;
}
