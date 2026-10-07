// The first screen: how the review that is about to be made is to be made.
// Shown in place of the review while there is none (`diffnote review` with
// nothing said); once 「レビューを作る」 is pressed and the server has made
// it, the page is loaded again, and is the review.
import { useEffect, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { server } from '../transport.ts';
import type { Answer, ReviewKind, SetupData, SetupPreview, SetupRaw, SetupRepo, SetupSpan } from '../model.ts';
import { MANUAL, baseFor, baseOf, candidateLabel, choiceOf, previewText, problemOf, rangeText, spanText, stateOf, targetOf, totalsOf, withRepo } from './setup.ts';
import { CommitGraph } from './SetupGraph.tsx';
import type { RepoRow, SetupState } from './setup.ts';

const KINDS: ReviewKind[] = ['git', 'workspace', 'raw'];

interface BaseProps {
  row: RepoRow;
  /** The row's name in the page (a review of several has many). */
  id: string;
  /** Asks the server about a commit typed in as a base. */
  preview(path: string, rev: string): Promise<Answer<{ preview: SetupPreview }>>;
  /** Asks the server what comparing from a base up to a target takes in. */
  span(path: string, base: string, target: string): Promise<Answer<{ span: SetupSpan }>>;
  /** Changes some of the row. A change is merged into the row as it is
   * then, not written over it: an answer from the server and a keystroke
   * can land in either order without one losing the other. */
  patch(part: Partial<RepoRow>): void;
}

// Which commit a repository is reviewed from: one of the ones offered, or
// one typed in, which the server is asked about as it is typed.
export function BasePicker(props: BaseProps) {
  var row = props.row;
  var latest = useRef(0);
  var ask = function (rev: string) {
    var n = ++latest.current;
    // (Typing there is choosing it.)
    if (!rev.trim()) { props.patch({ base: MANUAL, manual: rev, preview: null, error: '', checking: false }); return; }
    props.patch({ base: MANUAL, manual: rev, preview: null, error: '', checking: true });
    props.preview(row.info.path, rev.trim())
      .then(function (res) {
        if (latest.current !== n) return;
        props.patch({
          checking: false,
          preview: res.ok ? res.preview : null,
          error: res.ok ? '' : res.error,
        });
      });
  };
  return <div class="diffnote-setup__base" data-diffnote-setup-base={props.id}>
    {row.info.candidates.map(function (c) {
      // The commit, then why it is offered and what it would review, faint;
      // what the commit says is the tooltip.
      return <label key={c.rev} class="diffnote-setup__option">
        <input type="radio" name={'base-' + props.id} value={c.rev} checked={row.base === c.rev}
          onChange={function () { props.patch({ base: c.rev }); }} />
        <span><code title={c.subject}>{c.short}</code>
          <span class="diffnote-setup__why">{candidateLabel(c)}{lib.m('ui.setup.why_join')}{previewText(c.files)}</span></span>
      </label>;
    })}
    <label class="diffnote-setup__option">
      <input type="radio" name={'base-' + props.id} value={MANUAL} checked={row.base === MANUAL}
        onChange={function () { props.patch({ base: MANUAL }); }} />
      <span>{lib.m('ui.setup.manual')}
        <input type="text" class="diffnote-setup__rev" data-diffnote-setup-rev placeholder={lib.m('ui.setup.manual_placeholder')} value={row.manual}
          onFocus={function () { if (row.base !== MANUAL) props.patch({ base: MANUAL }); }}
          onInput={function (e) { ask(e.currentTarget.value); }} />
        {row.base === MANUAL && <small data-diffnote-setup-preview class={row.error ? 'diffnote-error' : ''}>
          {row.checking ? lib.m('ui.setup.checking')
            : row.error ? row.error
            : row.preview ? <><code title={row.preview.subject}>{row.preview.short}</code>
                <span class="diffnote-setup__why">{row.preview.subject}{lib.m('ui.setup.why_join')}{previewText(row.preview.files)}</span></>
            : ''}
        </small>}
      </span>
    </label>
  </div>;
}

/** Keeps a row told what comparing from its base up to its target takes
 * in, asking the server again whenever either changes. */
function useSpan(row: RepoRow, span: BaseProps['span'], patch: BaseProps['patch']) {
  var latest = useRef(0);
  var base = baseOf(row);
  var t = row.target.trim();
  // (A base typed in is asked about only once the server has said it is one.)
  var ready = !!base && (row.base !== MANUAL || !!row.preview);
  useEffect(function () {
    var n = ++latest.current;
    if (!ready || !t) {
      patch({ targetPreview: null, targetError: '', targetChecking: false });
      return;
    }
    patch({ targetPreview: null, targetError: '', targetChecking: true });
    span(row.info.path, base, t).then(function (res) {
      if (latest.current !== n) return;
      patch({
        targetChecking: false,
        targetPreview: res.ok ? res.span : null,
        targetError: res.ok ? '' : res.error,
      });
    });
  }, [ready, base, t]);
}

/** What comparing takes in, said beside the target. */
function SpanSaid(props: { row: RepoRow }) {
  var row = props.row;
  if (row.targetChecking) return <>{lib.m('ui.setup.checking')}</>;
  if (row.targetError) return <>{row.targetError}</>;
  if (!row.targetPreview) return null;
  return <><code title={row.targetPreview.subject}>{row.targetPreview.short}</code>
    <span class="diffnote-setup__why">{row.targetPreview.subject}{lib.m('ui.setup.why_join')}{spanText(row.targetPreview.commits)}</span></>;
}

/** What a repository is compared up to each time: `HEAD`, or a branch or
 * commit typed in, with what comparing from the base up to it takes in. */
export function TargetField(props: BaseProps) {
  var row = props.row;
  useSpan(row, props.span, props.patch);
  return <label class="diffnote-field diffnote-setup__target" data-diffnote-setup-target={props.id}>
    <span>{lib.m('ui.setup.target_label')}<small>{lib.m('ui.setup.target_hint')}</small></span>
    <span class="diffnote-setup__target-row">
      <input type="text" class="diffnote-setup__rev" data-diffnote-setup-target-rev value={row.target}
        onInput={function (e) { props.patch({ target: e.currentTarget.value }); }} />
      <small class={row.targetError ? 'diffnote-error' : ''} data-diffnote-setup-target-preview><SpanSaid row={row} /></small>
    </span>
  </label>;
}

export function RepoHead(props: { info: SetupRepo }) {
  var info = props.info;
  return <p class="diffnote-setup__repo-head">
    {info.branch ? lib.mf('ui.setup.on_branch', { branch: info.branch }) : lib.m('ui.setup.detached')}
    {' · '}
    {lib.mf('ui.setup.head', { short: info.head.short, subject: info.head.subject })}
  </p>;
}

/** Keeps a row of the first screen told what its range takes in (see
 * `useSpan` for the 「リポジトリ」 screen's): asked again whenever where it
 * starts or ends changes. Draws nothing. */
function RangeWatch(props: { path: string; base: string; target: string; patch(part: Partial<RepoRow>): void }) {
  var latest = useRef(0);
  useEffect(function () {
    var n = ++latest.current;
    if (!props.base || !props.target) {
      props.patch({ span: null, spanError: '', spanChecking: false });
      return;
    }
    props.patch({ spanError: '', spanChecking: true });
    server().get<{ span: SetupSpan }>('api/setup/preview?repo=' + encodeURIComponent(props.path) + '&rev=' + encodeURIComponent(props.target) + '&from=' + encodeURIComponent(props.base))
      .then(function (res) {
        if (latest.current !== n) return;
        props.patch({ spanChecking: false, span: res.ok ? res.span : null, spanError: res.ok ? '' : res.error });
      });
  }, [props.path, props.base, props.target]);
  return null;
}

/** What a row's range takes in, said in a few words. */
function RangeSaid(props: { row: RepoRow }) {
  var row = props.row;
  if (row.spanError) return <span class="diffnote-error">{row.spanError}</span>;
  if (row.spanChecking || !row.span) return <>{lib.m('ui.setup.checking')}</>;
  return <>{rangeText(row.span)}</>;
}

/** Where a repository's review starts and ends, side by side: what is
 * chosen for all (or, typed in, for this one alone), and what each review
 * goes up to. */
function RangeFields(props: { state: SetupState; row: RepoRow; canTarget: boolean; names: string[]; patch(part: Partial<RepoRow>): void }) {
  var row = props.row;
  var list = 'diffnote-setup-names-' + (row.info.path || 'one').replace(/[^A-Za-z0-9_-]/g, '_');
  var base = baseFor(props.state, row);
  var why = row.custom ? lib.m('ui.setup.from_custom')
    : props.state.bulk.mode === 'last' && props.state.kind === 'workspace' ? lib.mf('ui.setup.from_last', { n: String(props.state.bulk.count) })
    : lib.m('ui.setup.from_' + row.info.suggested.why);
  return <div class="diffnote-setup__range" data-diffnote-setup-range={row.info.path}>
    <label class="diffnote-field">
      <span>{lib.m('ui.setup.from_label')}<small>{why}</small></span>
      <input type="text" class="diffnote-setup__rev" data-diffnote-setup-from
        value={row.custom ? row.manual : (base === row.info.suggested.id ? row.info.suggested.short : base)}
        onInput={function (e) { props.patch({ custom: true, base: MANUAL, manual: e.currentTarget.value }); }} />
    </label>
    <label class="diffnote-field">
      <span>{lib.m('ui.setup.to_label')}<small>{lib.m(props.canTarget ? 'ui.setup.to_hint' : 'ui.setup.to_head_only')}</small></span>
      <input type="text" class="diffnote-setup__rev" data-diffnote-setup-to value={props.canTarget ? row.target : 'HEAD'} disabled={!props.canTarget}
        list={props.canTarget ? list : undefined}
        onInput={function (e) { props.patch({ target: e.currentTarget.value }); }} />
      {props.canTarget && <datalist id={list}>{props.names.map(function (n) { return <option key={n} value={n} />; })}</datalist>}
    </label>
    <p class="diffnote-setup__said" data-diffnote-setup-said>
      <RangeSaid row={row} />
      {row.custom && props.state.kind === 'workspace' && <button type="button" class="diffnote-link-button" data-diffnote-setup-uncustom
        onClick={function () { props.patch({ custom: false }); }}>{lib.m('ui.setup.uncustom')}</button>}
    </p>
  </div>;
}

/** The first screen, once what it is drawn from has come (finding the
 * repositories of a large directory takes a while: the page is shown first,
 * saying that it is being read). */
export function SetupLoader() {
  var _d = useState<SetupData | null>(null);
  var setup = _d[0];
  var setSetup = _d[1];
  var _e = useState<string | null>(null);
  var error = _e[0];
  var setError = _e[1];
  useEffect(function () {
    server().get<{ setup: SetupData }>('api/setup').then(function (res) {
      if (res.ok) setSetup(res.setup);
      else setError(res.error || lib.m('ui.setup.load_failed'));
    });
  }, []);
  if (setup) return <SetupScreen setup={setup} />;
  return <main class="diffnote-screen diffnote-setup" data-diffnote-setup-loading>
    <div class="diffnote-setup__layout">
      <div class="diffnote-setup__form">
        <h2>{lib.m('ui.setup.heading')}</h2>
        {error ? <p class="diffnote-error" role="alert">{error}</p>
          : <p class="diffnote-screen__note diffnote-setup__loading"><span class="diffnote-spinner" aria-hidden="true" />{lib.m('ui.setup.loading')}</p>}
      </div>
    </div>
  </main>;
}

export function SetupScreen(props: { setup: SetupData }) {
  var setup = props.setup;
  var _s = useState<SetupState>(function () { return stateOf(setup); });
  var state = _s[0];
  var setState = _s[1];
  var change = function (part: Partial<SetupState>) { setState(function (cur) { return Object.assign({}, cur, part); }); };
  // The names each repository's graph has (`HEAD`, branches, tags), for the
  // field where its review ends to offer.
  var _n = useState<Record<string, string[]>>({});
  var names = _n[0];
  var setNames = _n[1];
  var _k = useState(false);
  var choosingKind = _k[0];
  var setChoosingKind = _k[1];
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
  // The directory's files are counted when they are asked about (a big
  // directory takes a moment, and a review of commits never needs it).
  useEffect(function () {
    if (state.kind !== 'raw' || raw) return;
    server().get<{ raw: SetupRaw }>('api/setup/raw').then(function (res) { if (res.ok) setRaw(res.raw); });
  }, [state.kind]);
  // A part of a row, merged into the row as it is then (an answer from the
  // server and a keystroke can land in either order without one losing the
  // other).
  var patchRepo = function (path: string, part: Partial<RepoRow>) {
    setState(function (cur) {
      return Object.assign({}, cur, { repos: cur.repos.map(function (r) { return r.info.path === path ? Object.assign({}, r, part) : r; }) });
    });
  };
  var patchGit = function (part: Partial<RepoRow>) {
    setState(function (cur) { return cur.git ? Object.assign({}, cur, { git: Object.assign({}, cur.git, part) }) : cur; });
  };
  var add = function () {
    var path = addPath.trim();
    if (!path) return;
    setAddError('');
    server().get<{ repo: SetupRepo }>('api/setup/repo?path=' + encodeURIComponent(path)).then(function (res) {
      if (!res.ok) { setAddError(res.error); return; }
      var repos = withRepo(state.repos, res.repo);
      if (!repos) { setAddError(lib.mf('ui.setup.add_twice', { path: res.repo.path })); return; }
      change({ repos: repos, active: res.repo.path });
      setAddPath('');
    });
  };
  var problem = problemOf(state);
  var totals = totalsOf(state);
  var submit = function (e: Event) {
    e.preventDefault();
    if (busy || problem) return;
    setBusy(true);
    setError('');
    server().post<{ message: string }>('api/setup', choiceOf(state)).then(function (res) {
      if (res.ok) { location.reload(); return; }
      setBusy(false);
      setError(res.error || lib.m('ui.save_failed'));
    });
  };
  // The repository whose history is beside the form, and how to change its range.
  var activeRow = state.kind === 'git' ? state.git
    : state.kind === 'workspace' ? state.repos.filter(function (r) { return r.info.path === state.active; })[0] || null
    : null;
  var patchActive = function (part: Partial<RepoRow>) {
    if (!activeRow) return;
    if (state.kind === 'git') patchGit(part);
    else patchRepo(activeRow.info.path, part);
  };
  return <main class="diffnote-screen diffnote-setup" data-diffnote-setup>
    <form class="diffnote-setup__layout" noValidate onSubmit={submit}>
      <div class="diffnote-setup__form">
        <h2>{lib.m('ui.setup.heading')}</h2>
        <p class="diffnote-screen__note">{lib.mf('ui.setup.intro', { review: setup.review })}</p>

        {/* What it is of: as the directory looks, said in a line; changed when asked. */}
        <p class="diffnote-setup__kind-line" data-diffnote-setup-kind-line>
          {lib.mf('ui.setup.kind_line', { kind: lib.m('ui.setup.kind_' + state.kind) })}
          <button type="button" class="diffnote-link-button" data-diffnote-setup-kind-change aria-expanded={choosingKind}
            onClick={function () { setChoosingKind(!choosingKind); }}>{lib.m(choosingKind ? 'ui.setup.kind_done' : 'ui.setup.kind_change')}</button>
        </p>
        {choosingKind && <fieldset class="diffnote-setup__group">
          <legend>{lib.m('ui.setup.kind_label')}</legend>
          {KINDS.map(function (kind) {
            var can = setup.kinds[kind];
            return <label key={kind} class={'diffnote-setup__option' + (can.ok ? '' : ' is-off')}>
              <input type="radio" name="kind" value={kind} data-diffnote-setup-kind={kind} checked={state.kind === kind} disabled={!can.ok}
                onChange={function () { change({ kind: kind }); }} />
              <span>{lib.m('ui.setup.kind_' + kind)}<small>{can.ok ? lib.m('ui.setup.kind_' + kind + '_hint') : can.why}</small></span>
            </label>;
          })}
        </fieldset>}

        {state.kind === 'git' && state.git && <fieldset class="diffnote-setup__group" data-diffnote-setup-git>
          <legend>{lib.m('ui.setup.range_label')}</legend>
          <RepoHead info={state.git.info} />
          <RangeWatch path="" base={baseFor(state, state.git)} target="HEAD" patch={patchGit} />
          <RangeFields state={state} row={state.git} canTarget={false} names={[]} patch={patchGit} />
        </fieldset>}

        {state.kind === 'workspace' && <>
          <fieldset class="diffnote-setup__group" data-diffnote-setup-bulk>
            <legend>{lib.m('ui.setup.bulk_label')}</legend>
            <label class="diffnote-setup__option">
              <input type="radio" name="bulk" data-diffnote-setup-bulk-mode="suggested" checked={state.bulk.mode === 'suggested'}
                onChange={function () { change({ bulk: Object.assign({}, state.bulk, { mode: 'suggested' }) }); }} />
              <span>{lib.m('ui.setup.bulk_suggested')}<small>{lib.m('ui.setup.bulk_suggested_hint')}</small></span>
            </label>
            <label class="diffnote-setup__option">
              <input type="radio" name="bulk" data-diffnote-setup-bulk-mode="last" checked={state.bulk.mode === 'last'}
                onChange={function () { change({ bulk: Object.assign({}, state.bulk, { mode: 'last' }) }); }} />
              <span class="diffnote-setup__last">{lib.m('ui.setup.bulk_last_before')}
                <input type="number" min={1} max={999} class="diffnote-setup__count" data-diffnote-setup-bulk-count value={state.bulk.count}
                  onFocus={function () { if (state.bulk.mode !== 'last') change({ bulk: Object.assign({}, state.bulk, { mode: 'last' }) }); }}
                  onInput={function (e) { change({ bulk: { mode: 'last', count: Math.max(1, Math.min(999, Number(e.currentTarget.value) || 1)) } }); }} />
                {lib.m('ui.setup.bulk_last_after')}</span>
            </label>
            <p class="diffnote-screen__note">{lib.m('ui.setup.bulk_note')}</p>
          </fieldset>

          <fieldset class="diffnote-setup__group" data-diffnote-setup-repos>
            <legend>{lib.m('ui.setup.repos_label')}</legend>
            <p class="diffnote-screen__note">{lib.m('ui.setup.repos_note')}</p>
            {state.repos.map(function (row) {
              var path = row.info.path;
              var active = state.active === path;
              var patch = function (part: Partial<RepoRow>) { patchRepo(path, part); };
              // One line each: whether it is in, its name and branch, what
              // it would review. The one chosen opens, with its range, and
              // its history is beside the form.
              return <div key={path} class={'diffnote-setup__repo' + (row.on ? ' is-on' : ' is-off') + (active ? ' is-active' : '')} data-diffnote-setup-repo={path}
                onClick={function () { if (!active) change({ active: path }); }}>
                <RangeWatch path={path} base={baseFor(state, row)} target={targetOf(row)} patch={patch} />
                <div class="diffnote-setup__repo-line">
                  <input type="checkbox" data-diffnote-setup-include checked={row.on} aria-label={path}
                    onChange={function (e) { patch({ on: e.currentTarget.checked }); change({ active: path }); }} />
                  <code class="diffnote-setup__repo-name">{path}</code>
                  <span class="diffnote-setup__branch">{row.info.branch || lib.m('ui.setup.detached')}</span>
                  {row.custom && <span class="diffnote-setup__badge" data-diffnote-setup-custom>{lib.m('ui.setup.custom_badge')}</span>}
                  <span class="diffnote-setup__summary" data-diffnote-setup-summary><RangeSaid row={row} /></span>
                </div>
                {active && <RangeFields state={state} row={row} canTarget={true} names={names[path] || ['HEAD']} patch={patch} />}
              </div>;
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
          </fieldset>
        </>}

        {state.kind === 'raw' && <fieldset class="diffnote-setup__group" data-diffnote-setup-raw>
          <legend>{lib.m('ui.setup.raw_label')}</legend>
          <p class="diffnote-screen__note" data-diffnote-setup-raw-note>{raw
            ? lib.mf('ui.setup.raw_files', { files: String(raw.files), size: lib.formatSize(raw.bytes) })
            : lib.m('ui.setup.raw_loading')}</p>
        </fieldset>}

        <label class="diffnote-field">
          <span>{lib.m('ui.setup.title_label')}</span>
          <input type="text" maxlength={200} data-diffnote-setup-title value={state.title} placeholder={lib.m('ui.setup.title_placeholder')}
            onInput={function (e) { change({ title: e.currentTarget.value }); }} />
        </label>

        {state.kind !== 'raw' && <details class="diffnote-setup__more" data-diffnote-setup-more>
          <summary>{lib.m('ui.setup.more_label')}</summary>
          <fieldset class="diffnote-setup__group">
            <legend>{lib.m('ui.setup.snapshot_label')}</legend>
            {(['changed', 'full'] as const).map(function (mode) {
              return <label key={mode} class="diffnote-setup__option">
                <input type="radio" name="snapshot" value={mode} data-diffnote-setup-snapshot={mode} checked={state.snapshot === mode}
                  onChange={function () { change({ snapshot: mode }); }} />
                <span>{lib.m('ui.setup.snapshot_' + mode)}</span>
              </label>;
            })}
            <p class="diffnote-screen__note">{lib.m('ui.setup.snapshot_hint')}</p>
          </fieldset>
        </details>}
      </div>

      {activeRow && <aside class="diffnote-setup__side">
        <CommitGraph repo={activeRow.info.path} span={activeRow.span} canTarget={state.kind === 'workspace'}
          onFrom={function (base) { patchActive({ custom: true, base: MANUAL, manual: base.slice(0, 12) }); }}
          onTo={function (target) { patchActive({ target: /^[0-9a-f]{40}$/.test(target) ? target.slice(0, 12) : target }); }}
          onNames={function (list) { var path = activeRow!.info.path; setNames(function (cur) { return Object.assign({}, cur, { [path]: list }); }); }} />
      </aside>}

      {/* What the review takes in, and the button that makes it: always in view. */}
      <div class="diffnote-setup__foot">
        {error && <p class="diffnote-error" data-diffnote-setup-error role="alert">{error}</p>}
        {state.kind !== 'raw' && totals.repos > 0 && <span class="diffnote-setup__totals" data-diffnote-setup-totals>
          {state.kind === 'workspace'
            ? lib.mf('ui.setup.totals', { repos: String(totals.repos), commits: String(totals.commits), files: String(totals.files) })
            : lib.mf('ui.setup.totals_one', { commits: String(totals.commits), files: String(totals.files) })}</span>}
        {problem && <span class="diffnote-screen__dirty" data-diffnote-setup-problem>{lib.m(problem)}</span>}
        <button type="submit" class="diffnote-button diffnote-button--primary" data-diffnote-setup-create disabled={busy || !!problem}>
          {busy ? lib.m('ui.setup.creating') : lib.m('ui.setup.create_button')}</button>
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
