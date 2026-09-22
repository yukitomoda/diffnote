// The page: a Preact app drawn from the data in `#diffnote-data` (see
// `src/html/viewmodel.rs` for its form).
//
// The markup (class names, data attributes) is what `ui/src/style.css` styles
// and what `ui/src/interact.ts` and the browser tests look for.
import { render } from 'preact';
import { useStore } from '@nanostores/preact';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import { interact } from './interact.ts';
import { lib } from './lib.ts';
import { server } from './transport.ts';
import { Revision } from './Revision.tsx';
import { QuitButton } from './settings/Quit.tsx';
import { SettingsScreen } from './settings/Screen.tsx';
import { useCompose } from './state/compose.ts';
import { ActionsContext, ComposeContext, LinksContext, OpenedContext } from './state/contexts.ts';
import { useOpened } from './state/opened.ts';
import { useReview } from './state/review.ts';
import {
  compareWith,
  followRoute,
  goTo,
  openRevision,
  route,
  routeOfHash,
  showRevision,
  showRoute,
  showScreen,
  startRoute,
} from './state/route.ts';
import { isViewed, seen, toggleViewed } from './state/viewed.ts';
import {
  hideResolved,
  ignoreWhitespace,
  layout as shownLayout,
  startView,
  watchWidth,
} from './state/view.ts';
import type { At } from './lib.ts';
import type { RevisionData, ViewModel } from './model.ts';
import type { Links } from './state/contexts.ts';
import type { PullNote } from './settings/General.tsx';
import { Icon } from './icon.tsx';

function App(props: { model: ViewModel }) {
  var review = useReview(props.model);
  var openedFiles = useOpened(props.model.interactive);
  var model = review.model;
  // Where the page is: the revision shown, the settings screen, what it is
  // compared against and the last place jumped to, all of which are in the
  // address (see `state/route.ts`).
  var here = useStore(route);
  var current = here.rev;
  var screen = here.screen;
  var against = here.against;
  var _k = useState<{ rev: number; from: number; data: RevisionData } | null>(null);
  var cmp = _k[0];
  var setCmp = _k[1];
  useEffect(function () {
    if (against != null && against >= current) compareWith(null);
  }, [current, against]);
  useEffect(function () {
    if (!review.actions || against == null || against >= current) { setCmp(null); return undefined; }
    var stale = false;
    server().get<{ revision: RevisionData }>('/api/compare?rev=' + current + '&from=' + against).then(function (res) {
      if (stale) return;
      if (res.ok) setCmp({ rev: current, from: against!, data: res.revision });
      else { setCmp(null); compareWith(null); }
    });
    return function () { stale = true; };
  }, [against, current, model.stamp]);
  // What the page draws for the revision then: the same files, but a file that
  // is marked as looked at is the same one (its text is this revision's).
  var override = useMemo(function () {
    if (!cmp || cmp.rev !== current || cmp.from !== against) return null;
    var sigs: Record<string, string | undefined> = {};
    model.revisions[current].files.forEach(function (f) { sigs[f.path] = f.sig; });
    return Object.assign({}, cmp.data, {
      files: cmp.data.files.map(function (f) {
        return Object.prototype.hasOwnProperty.call(sigs, f.path) ? Object.assign({}, f, { sig: sigs[f.path] }) : f;
      }),
    });
  }, [cmp, current, against, model]);
  // 添付 lists what the bundle holds, and an upload's answer says only how
  // big the bundle now is (not a whole model): ask for one when that screen
  // opens, so what was just attached is in the list.
  useEffect(function () {
    if (screen === 'attachments' && review.reload) review.reload();
  }, [screen === 'attachments']);
  // The tab that is shown is kept in view when there are more than fit.
  var tabs = useRef<HTMLElement | null>(null);
  useLayoutEffect(function () {
    var nav = tabs.current;
    var shown = nav && nav.querySelector<HTMLElement>('a.is-current');
    if (nav && shown) nav.scrollLeft = shown.offsetLeft - (nav.clientWidth - shown.offsetWidth) / 2;
  }, [current, model.revisions.length]);
  var links: Links = useMemo(function (): Links {
    // A path is a place if some revision has the file.
    var known: Record<string, boolean> = {};
    model.revisions.forEach(function (r) { r.files.forEach(function (f) { known[f.path] = true; }); });
    // Opens a file, some of its lines, or a thread, in revision `rev` (bringing
    // back a file marked "viewed" first, since it would otherwise be hidden).
    // Used for a live jump, and to retrace one from the address alike.
    var jump = function (rev: number, place: At) {
      var revision = model.revisions[rev];
      if (!revision) return;
      var of = place.kind === 'thread' ? revision.placements[place.id] : null;
      var path = place.kind === 'thread' ? (of && 'file' in of ? of.file : null) : place.path;
      var file = path && revision.files.filter(function (f) { return f.path === path; })[0];
      if (file && isViewed(file, seen.get())) toggleViewed(file);
      if (place.kind === 'file') interact.showFile(rev, place.path);
      else if (place.kind === 'thread') interact.jumpWhenShown('r' + rev + '-thread-' + place.id);
      else interact.showLines(rev, place.path, place.side, place.start, place.end);
    };
    return {
      current: current,
      revisions: model.revisions.length,
      has: function (path) { return Object.prototype.hasOwnProperty.call(known, path); },
      // The most a file attached to a comment may weigh (the review's rule).
      limit: model.attachment_limit,
      // Where another attached file is: in the page, or at the server. One
      // the 添付 screen has taken out is asked for all the same (the comment
      // is left as written): the link is simply broken from then on.
      file: function (id, name) {
        if (model.attachments && model.attachments[id]) return model.attachments[id];
        return model.interactive ? '/api/attachments/' + id + '?name=' + encodeURIComponent(name) : '';
      },
      // Where an image is: in the page (an exported one), or at the server.
      image: function (id) {
        if (model.images && model.images[id]) return model.images[id];
        return model.interactive ? '/api/images/' + id : '';
      },
      // A place chosen by clicking: a line reference (`{path,side,start,end,rev}`,
      // as `lib.lineRefs` gives them), or `{kind:'file'|'thread', ...}`. Also
      // remembered, so the browser's back/forward can retrace the jump.
      go: function (ref) {
        // A line reference names its revision as the tabs do (1-based), or not
        // at all; a place already says what kind it is.
        var index = 'kind' in ref ? current : ref.rev == null ? current : ref.rev - 1;
        var place: At = 'kind' in ref
          ? ref
          : { kind: 'lines', path: ref.path, side: ref.side, start: ref.start, end: ref.end };
        goTo(index, place);
        jump(index, place);
      },
      jump: jump,
    };
  }, [model, current]);
  // The address may already point at a specific place (from a copied link, or
  // typed in): jump there once the page has drawn.
  useEffect(function () {
    var start = startRoute(location.hash, model.revisions.length);
    if (start && start.at) links.jump(start.rev - 1, start.at);
  }, []);
  // The browser's back/forward buttons: retrace the revision, settings screen,
  // compare target and last jump, exactly as the address says.
  useEffect(function () {
    var onPop = function () {
      var there = routeOfHash(location.hash, model.revisions.length);
      if (!there) return;
      showRoute(there);
      if (there.at) links.jump(there.rev - 1, there.at);
    };
    window.addEventListener('popstate', onPop);
    return function () { window.removeEventListener('popstate', onPop); };
  }, [model.revisions.length, links]);
  // The other way around: what changed here is written to the address (so
  // the buttons above have something to retrace), unless it came from there
  // just now. The very first time, the address is only filled in, not added
  // to (nothing was navigated to yet -- it is where the page already was).
  // The address says where the page is, for as long as it is open.
  useEffect(followRoute, []);
  // Differences that are only in white space ignored: the review says how the page
  // starts (a default kept in it), and changing it here is for this page only.
  // What pressing 「最新を取り込む」 did, said next to it.
  var _n = useState<PullNote | null>(null);
  var note = _n[0];
  var setNote = _n[1];
  var pull = function () {
    setNote({ text: lib.m('ui.topbar.pull_note_loading'), busy: true });
    review.actions!.refresh().then(function (res) {
      setNote({ text: (res.ok ? res.message : res.error) || lib.m('ui.topbar.pull_failed'), failed: !res.ok });
      // What was taken in is what to look at now.
      if (res.ok && res.added && res.model) showRevision(res.model.revisions.length - 1);
    });
  };
  // Short messages at the top of the page (see TopbarNotices): today, only
  // a pull waiting to be taken in makes one, and it opens 全般 (where the
  // button now lives) rather than acting by itself.
  var notices: Notice[] = [];
  if (review.actions && model.refreshable && review.pending) {
    notices.push({
      id: 'pending',
      text: lib.m('ui.topbar.new_commits_notice'),
      onClick: function () { showScreen('general'); },
    });
  }
  var hide = useStore(hideResolved);
  var ignoreSpace = useStore(ignoreWhitespace);
  var layout = useStore(shownLayout);
  // Two columns need the room, for as long as the page is open.
  useEffect(watchWidth, []);
  // What was chosen or written belongs to the revision and layout it was in.
  var compose = useCompose(review.actions, current + ':' + layout);

  // At once (not after the next paint): the style that hides cards hangs on it.
  useLayoutEffect(function () {
    document.body.classList.toggle('diffnote-hide-resolved', hide);
    interact.reset();
  }, [hide, current, layout]);

  return <article class="diffnote-review">
    <div class="diffnote-topbar">
      <header class="diffnote-summary">
        <h1>{review.actions
          ? <button type="button" class="diffnote-title" data-diffnote-settings title={lib.m('ui.settings.title_button')} aria-haspopup="dialog"
              aria-pressed={screen === 'general' || screen === 'settings'} onClick={function () { showScreen(screen === 'general' ? null : 'general'); }}>{model.title || lib.m('html.default_title')}<Icon name="settings" class="diffnote-title__icon" /></button>
          : model.title || lib.m('html.default_title')}</h1>
        {model.base && <p data-diffnote-base class={against != null ? 'is-changed' : ''} title={against != null ? lib.m('ui.base.changed_title') : lib.m('ui.base.default_title')}>{lib.m('ui.base.label')}: {review.actions && current > 0
          ? <select class="diffnote-base__select" data-diffnote-base-select aria-label={lib.m('ui.base.select_label')} value={against == null ? '' : String(against)}
              onChange={function (e) { compareWith(e.currentTarget.value === '' ? null : +e.currentTarget.value); }}>
              <option value="">{model.base.kind === 'git' ? model.base.id : lib.formatTime(model.base.at)}</option>
              {model.revisions.slice(0, current).map(function (r, i) { return <option key={i} value={String(i)}>{r.label} ({lib.formatRecorded(r.at)})</option>; })}
            </select>
          : model.base.kind === 'git' ? <code>{model.base.id}</code> : lib.formatTime(model.base.at)}</p>}
      </header>
      {model.revisions.length > 0 && <nav class="diffnote-revisions" ref={tabs} onWheel={function (e) {
        // The tabs scroll sideways (no bar is shown): the wheel does it too.
        if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) { e.currentTarget.scrollLeft += e.deltaY; e.preventDefault(); }
      }}><ul>
        {model.revisions.map(function (r, i) {
          return <li key={i}><a href={'#rev-' + i} data-diffnote-revision-link={i} class={i === current ? 'is-current' : ''}
            onClick={function (e) { e.preventDefault(); openRevision(i); }}>{r.label} <span class="diffnote-revisions__at">({lib.formatRecorded(r.at)})</span></a></li>;
        })}
      </ul></nav>}
      <div class="diffnote-topbar__actions">
      <TopbarNotices items={notices} />
      {model.interactive && <QuitButton />}
      </div>
    </div>
    {screen != null && review.actions && <SettingsScreen section={screen} model={model} pending={review.pending} note={note} onPull={pull}
      saveSettings={review.actions.saveSettings} saveUserSettings={review.actions.saveUserSettings}
      removeAttached={review.actions.removeAttached}
      onShowThread={function (id) { links.go({ kind: 'thread', id: id }); }}
      placementOf={function (id) { return ((model.revisions[current] || {}).placements || {})[id]; }}
      onSelect={showScreen} onClose={function () { showScreen(null); }} />}
    <div class="diffnote-review-body" hidden={screen != null && !!review.actions}>
    <LinksContext.Provider value={links}>
    <ActionsContext.Provider value={review.actions}>
      <ComposeContext.Provider value={compose}>
        <OpenedContext.Provider value={openedFiles}>
          <Revision key={current} model={model} index={current} hideResolved={hide} layout={layout} ignoreSpace={ignoreSpace} compose={compose} override={override}
            overrideNote={override ? {
              short: model.revisions[against!].label + ' .. ' + model.revisions[current].label,
              tip: lib.m('ui.base.select_tip'),
            } : null}
            author={review.actions ? model.author : null} userSettingsOpen={screen === 'user'}
            onToggleUserSettings={review.actions && function () { showScreen(screen === 'user' ? null : 'user'); }} />
        </OpenedContext.Provider>
      </ComposeContext.Provider>
    </ActionsContext.Provider>
    </LinksContext.Provider>
    </div>
  </article>;
}

// Short, clickable messages at the top (today, only a pending pull makes
// one): built generic so another source can add its own later without new
// topbar markup, each just {id, text, onClick}.
interface Notice {
  id: string;
  text: string;
  onClick(): void;
}

function TopbarNotices(props: { items: Notice[] }) {
  if (!props.items || props.items.length === 0) return null;
  return <div class="diffnote-notices">
    {props.items.map(function (n: Notice) {
      return <button key={n.id} type="button" class="diffnote-notice" data-diffnote-notice={n.id} onClick={n.onClick}>{n.text}</button>;
    })}
  </div>;
}

export function start() {
  var read = function (id: string) {
    return JSON.parse(document.getElementById(id)!.textContent || '{}');
  };
  lib.setMessages(read('diffnote-messages'));
  var model: ViewModel = read('diffnote-data');
  startView(!!model.ignore_whitespace);
  if (model.interactive) document.body.setAttribute('data-diffnote-api', '1');
  interact.install();
  render(<App model={model} />, document.getElementById('app')!);
}
