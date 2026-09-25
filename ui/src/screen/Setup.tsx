// The first screen: how the review that is about to be made is to be made.
// Shown in place of the review while there is none (`diffnote review` with
// nothing said); once 「レビューを作る」 is pressed and the server has made
// it, the page is loaded again, and is the review.
import { useEffect, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { server } from '../transport.ts';
import type { ReviewKind, SetupData, SetupPreview, SetupRaw, SetupRepo } from '../model.ts';
import { MANUAL, candidateLabel, choiceOf, previewText, problemOf, stateOf, withRepo } from './setup.ts';
import type { RepoRow, SetupState } from './setup.ts';

const KINDS: ReviewKind[] = ['git', 'workspace', 'raw'];

interface BaseProps {
  row: RepoRow;
  /** The row's name in the page (a review of several has many). */
  id: string;
  onChange(row: RepoRow): void;
}

// Which commit a repository is reviewed from: one of the ones offered, or
// one typed in, which the server is asked about as it is typed.
function BasePicker(props: BaseProps) {
  var row = props.row;
  var latest = useRef(0);
  var ask = function (rev: string) {
    var n = ++latest.current;
    // (Typing there is choosing it.)
    if (!rev.trim()) { props.onChange(Object.assign({}, row, { base: MANUAL, manual: rev, preview: null, error: '', checking: false })); return; }
    props.onChange(Object.assign({}, row, { base: MANUAL, manual: rev, preview: null, error: '', checking: true }));
    server().get<{ preview: SetupPreview }>('/api/setup/preview?repo=' + encodeURIComponent(row.info.path) + '&rev=' + encodeURIComponent(rev.trim()))
      .then(function (res) {
        if (latest.current !== n) return;
        // (What the row is by now, not what it was when the question was asked.)
        props.onChange(Object.assign({}, current.current, {
          checking: false,
          preview: res.ok ? res.preview : null,
          error: res.ok ? '' : res.error,
        }));
      });
  };
  var current = useRef(row);
  current.current = row;
  return <div class="diffnote-setup__base" data-diffnote-setup-base={props.id}>
    {row.info.candidates.map(function (c) {
      return <label key={c.rev} class="diffnote-setup__option">
        <input type="radio" name={'base-' + props.id} value={c.rev} checked={row.base === c.rev}
          onChange={function () { props.onChange(Object.assign({}, row, { base: c.rev })); }} />
        <span>{candidateLabel(c)} <code>{c.short}</code> <span class="diffnote-setup__subject">{c.subject}</span>
          <small>{previewText(c.commits, c.files)}</small></span>
      </label>;
    })}
    <label class="diffnote-setup__option">
      <input type="radio" name={'base-' + props.id} value={MANUAL} checked={row.base === MANUAL}
        onChange={function () { props.onChange(Object.assign({}, row, { base: MANUAL })); }} />
      <span>{lib.m('ui.setup.manual')}
        <input type="text" class="diffnote-setup__rev" data-diffnote-setup-rev placeholder={lib.m('ui.setup.manual_placeholder')} value={row.manual}
          onFocus={function () { if (row.base !== MANUAL) props.onChange(Object.assign({}, row, { base: MANUAL })); }}
          onInput={function (e) { ask(e.currentTarget.value); }} />
        {row.base === MANUAL && <small data-diffnote-setup-preview class={row.error ? 'diffnote-error' : ''}>
          {row.checking ? lib.m('ui.setup.checking')
            : row.error ? row.error
            : row.preview ? <>{row.preview.short} {row.preview.subject}: {previewText(row.preview.commits, row.preview.files)}</>
            : ''}
        </small>}
      </span>
    </label>
  </div>;
}

function RepoHead(props: { info: SetupRepo }) {
  var info = props.info;
  return <p class="diffnote-setup__repo-head">
    {info.branch ? lib.mf('ui.setup.on_branch', { branch: info.branch }) : lib.m('ui.setup.detached')}
    {' · '}
    {lib.mf('ui.setup.head', { short: info.head.short, subject: info.head.subject })}
  </p>;
}

export function SetupScreen(props: { setup: SetupData }) {
  var setup = props.setup;
  var _s = useState<SetupState>(function () { return stateOf(setup); });
  var state = _s[0];
  var setState = _s[1];
  var change = function (part: Partial<SetupState>) { setState(function (cur) { return Object.assign({}, cur, part); }); };
  var _r = useState<SetupRaw | null>(null);
  var raw = _r[0];
  var setRaw = _r[1];
  var _a = useState('');
  var addPath = _a[0];
  var setAddPath = _a[1];
  var _ae = useState('');
  var addError = _ae[0];
  var setAddError = _ae[1];
  var _b = useState(false);
  var busy = _b[0];
  var setBusy = _b[1];
  var _e = useState('');
  var error = _e[0];
  var setError = _e[1];
  var first = useRef<HTMLInputElement | null>(null);
  useEffect(function () { if (first.current) first.current.focus(); }, []);
  // The directory's files are counted when they are asked about (a big
  // directory takes a moment, and a review of commits never needs it).
  useEffect(function () {
    if (state.kind !== 'raw' || raw) return;
    server().get<{ raw: SetupRaw }>('/api/setup/raw').then(function (res) { if (res.ok) setRaw(res.raw); });
  }, [state.kind]);
  var setRepo = function (i: number, row: RepoRow) {
    setState(function (cur) {
      var repos = cur.repos.slice();
      repos[i] = row;
      return Object.assign({}, cur, { repos: repos });
    });
  };
  var add = function () {
    var path = addPath.trim();
    if (!path) return;
    setAddError('');
    server().get<{ repo: SetupRepo }>('/api/setup/repo?path=' + encodeURIComponent(path)).then(function (res) {
      if (!res.ok) { setAddError(res.error); return; }
      var repos = withRepo(state.repos, res.repo);
      if (!repos) { setAddError(lib.mf('ui.setup.add_twice', { path: res.repo.path })); return; }
      change({ repos: repos });
      setAddPath('');
    });
  };
  var problem = problemOf(state);
  var submit = function (e: Event) {
    e.preventDefault();
    if (busy || problem) return;
    setBusy(true);
    setError('');
    server().post<{ message: string }>('/api/setup', choiceOf(state)).then(function (res) {
      if (res.ok) { location.reload(); return; }
      setBusy(false);
      setError(res.error || lib.m('ui.save_failed'));
    });
  };
  var kindLabel = function (kind: ReviewKind) { return lib.m('ui.setup.kind_' + kind); };
  return <main class="diffnote-screen diffnote-setup" data-diffnote-setup>
    <form class="diffnote-screen__form diffnote-setup__form" noValidate onSubmit={submit}>
      <h2>{lib.m('ui.setup.heading')}</h2>
      <p class="diffnote-screen__note">{lib.mf('ui.setup.intro', { review: setup.review })}</p>

      <fieldset class="diffnote-setup__group">
        <legend>{lib.m('ui.setup.kind_label')}</legend>
        {KINDS.map(function (kind) {
          var can = setup.kinds[kind];
          return <label key={kind} class={'diffnote-setup__option' + (can.ok ? '' : ' is-off')}>
            <input type="radio" name="kind" value={kind} data-diffnote-setup-kind={kind} checked={state.kind === kind} disabled={!can.ok}
              onChange={function () { change({ kind: kind }); }} />
            <span>{kindLabel(kind)}<small>{can.ok ? lib.m('ui.setup.kind_' + kind + '_hint') : can.why}</small></span>
          </label>;
        })}
      </fieldset>

      {state.kind === 'git' && state.git && <fieldset class="diffnote-setup__group" data-diffnote-setup-git>
        <legend>{lib.m('ui.setup.base_label')}</legend>
        <RepoHead info={state.git.info} />
        <BasePicker row={state.git} id="git" onChange={function (row) { change({ git: row }); }} />
      </fieldset>}

      {state.kind === 'workspace' && <fieldset class="diffnote-setup__group" data-diffnote-setup-repos>
        <legend>{lib.m('ui.setup.repos_label')}</legend>
        <p class="diffnote-screen__note">{lib.mf('ui.setup.repos_note', { depth: String(setup.depth) })}</p>
        {state.repos.map(function (row, i) {
          return <details key={row.info.path} class={'diffnote-setup__repo' + (row.on ? '' : ' is-off')} data-diffnote-setup-repo={row.info.path} open={row.on}>
            <summary>
              <input type="checkbox" data-diffnote-setup-include checked={row.on} onClick={function (e) { e.stopPropagation(); }}
                onChange={function (e) { setRepo(i, Object.assign({}, row, { on: e.currentTarget.checked })); }} />
              <code>{row.info.path}</code>
              {row.on && <span class="diffnote-setup__summary">{row.base === MANUAL
                ? (row.preview ? row.preview.short : lib.m('ui.setup.manual'))
                : (row.info.candidates.filter(function (c) { return c.rev === row.base; })[0] || { short: '' }).short}</span>}
            </summary>
            {row.on && <>
              <RepoHead info={row.info} />
              <BasePicker row={row} id={'repo-' + i} onChange={function (changed) { setRepo(i, changed); }} />
            </>}
          </details>;
        })}
        <div class="diffnote-setup__add">
          <label class="diffnote-field">
            <span>{lib.m('ui.setup.add_path_label')}</span>
            <span class="diffnote-setup__add-row">
              <input type="text" data-diffnote-setup-add-path value={addPath} placeholder={lib.m('ui.setup.add_path_placeholder')}
                onInput={function (e) { setAddPath(e.currentTarget.value); setAddError(''); }}
                onKeyDown={function (e) { if (e.key === 'Enter') { e.preventDefault(); add(); } }} />
              <button type="button" class="diffnote-button" data-diffnote-setup-add disabled={!addPath.trim()} onClick={add}>{lib.m('ui.setup.add_button')}</button>
            </span>
          </label>
          {addError && <p class="diffnote-error" data-diffnote-setup-add-error role="alert">{addError}</p>}
        </div>
      </fieldset>}

      {state.kind === 'raw' && <fieldset class="diffnote-setup__group" data-diffnote-setup-raw>
        <legend>{lib.m('ui.setup.raw_label')}</legend>
        <p class="diffnote-screen__note" data-diffnote-setup-raw-note>{raw
          ? lib.mf('ui.setup.raw_files', { files: String(raw.files), size: lib.formatSize(raw.bytes) })
          : lib.m('ui.setup.raw_loading')}</p>
      </fieldset>}

      <label class="diffnote-field">
        <span>{lib.m('ui.setup.title_label')}</span>
        <input ref={first} type="text" maxlength={200} data-diffnote-setup-title value={state.title} placeholder={lib.m('ui.setup.title_placeholder')}
          onInput={function (e) { change({ title: e.currentTarget.value }); }} />
      </label>

      {state.kind !== 'raw' && <fieldset class="diffnote-setup__group">
        <legend>{lib.m('ui.setup.snapshot_label')}</legend>
        {(['changed', 'full'] as const).map(function (mode) {
          return <label key={mode} class="diffnote-setup__option">
            <input type="radio" name="snapshot" value={mode} data-diffnote-setup-snapshot={mode} checked={state.snapshot === mode}
              onChange={function () { change({ snapshot: mode }); }} />
            <span>{lib.m('ui.setup.snapshot_' + mode)}</span>
          </label>;
        })}
        <p class="diffnote-screen__note">{lib.m('ui.setup.snapshot_hint')}</p>
      </fieldset>}

      {error && <p class="diffnote-error" data-diffnote-setup-error role="alert">{error}</p>}
      <div class="diffnote-reply__buttons">
        <button type="submit" class="diffnote-button diffnote-button--primary" data-diffnote-setup-create disabled={busy || !!problem}>
          {busy ? lib.m('ui.setup.creating') : lib.m('ui.setup.create_button')}</button>
        {problem && <span class="diffnote-screen__dirty" data-diffnote-setup-problem>{lib.m(problem)}</span>}
      </div>
    </form>
  </main>;
}

/** A review with nothing to show yet: its base is recorded, and what comes
 * after it is what is reviewed. */
export function EmptyReview(props: { refreshable: boolean; busy: boolean; note: string | null; failed: boolean; onPull(): void }) {
  return <main class="diffnote-screen diffnote-empty" data-diffnote-empty>
    <h2>{lib.m('ui.empty.heading')}</h2>
    <p class="diffnote-screen__note">{lib.m(props.refreshable ? 'ui.empty.note' : 'ui.empty.note_open')}</p>
    {props.refreshable && <div class="diffnote-screen__action">
      <button type="button" class="diffnote-button diffnote-button--primary" data-diffnote-pull disabled={props.busy} onClick={props.onPull}>{lib.m('ui.settings.pull_button')}</button>
      {props.note && <p class={'diffnote-pull__note' + (props.failed ? ' is-failed' : '')} data-diffnote-pull-note role="status">{props.note}</p>}
    </div>}
  </main>;
}
