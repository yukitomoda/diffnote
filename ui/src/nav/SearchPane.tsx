// The search, in the left pane in place of the files and threads: a box,
// how many were found and which is the one gone to, and what was found,
// file by file, then the comments. Pressing one goes to it (a folded file
// is opened, one marked as looked at brought back); Enter and Shift+Enter go
// to the next and the one before. What was found is marked in the page
// itself as well, as far as the page has drawn it (`paint`).
import { useContext, useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { useStore } from '@nanostores/preact';
import { lib } from '../lib.ts';
import { LinksContext, OpenedContext } from '../state/contexts.ts';
import { searchCase, searchCurrent, searchFocus, searchText, showFiles } from '../state/search.ts';
import { reveal } from '../state/reveal.ts';
import { server, transport } from '../transport.ts';
import { SEARCH_ALL_TIME, carriedGapLines, excerpt, occurrences, placeOf, searchRevision } from '../search.ts';
import type { FileGroup, GapLine, Hit, OtherFound, SearchQuery, SearchResult, Span } from '../search.ts';
import type { FileData, Placement, ThreadData } from '../model.ts';
import { Icon } from '../icon.tsx';

export interface SearchPaneProps {
  rev: number;
  /** The files the revision shows (as the page has them, compared or not). */
  files: FileData[];
  threads: ThreadData[];
  placements: Record<string, Placement>;
}

/** How long typing has to stop before it is searched for (ms). */
var SETTLE = 300;

/** How many of what was found the list draws at a time (more as it is
 * scrolled to the end). */
var ROWS = 1000;

export function SearchPane(props: SearchPaneProps) {
  var links = useContext(LinksContext);
  var opened = useContext(OpenedContext);
  var text = useStore(searchText);
  var caseSensitive = useStore(searchCase);
  var current = useStore(searchCurrent);
  var focus = useStore(searchFocus);
  var box = useRef<HTMLInputElement | null>(null);
  useEffect(function () {
    if (box.current) { box.current.focus(); box.current.select(); }
  }, [focus]);
  // What is searched for: the text once typing stops.
  var _q = useState(text);
  var asked = _q[0];
  var setAsked = _q[1];
  useEffect(function () {
    var t = setTimeout(function () { setAsked(text); }, SETTLE);
    return function () { clearTimeout(t); };
  }, [text]);
  var query: SearchQuery = { text: asked, caseSensitive: caseSensitive };
  var wanted = asked ? asked + '\0' + caseSensitive + '\0' + props.rev : '';
  // Past the most it finds, it can be asked to find them all (for as long as
  // `SEARCH_ALL_TIME`, from when it was asked).
  var _w = useState<{ for: string; at: number } | null>(null);
  var whole = _w[0] && _w[0].for === wanted ? _w[0] : null;
  var setWhole = _w[1];
  var asking = wanted ? wanted + (whole ? '\0all' : '') : '';
  // The files' every line: the lines the diffs leave out too -- what an
  // exported page carries, or, served, what the server finds (and, there,
  // in the files the page doesn't show as well).
  var _g = useState<{ for: string; lines: GapLine[]; others?: OtherFound; cut: boolean } | null>(null);
  var gaps = _g[0];
  var setGaps = _g[1];
  // (Asked again when the files are others, not when the same are drawn anew.)
  var paths = props.files.map(function (f) { return f.path; });
  var pathsKey = paths.join('\n');
  useEffect(function () {
    if (!asking) { setGaps(null); return undefined; }
    if (!transport) { setGaps({ for: asking, lines: carriedGapLines(props.files), cut: false }); return undefined; }
    var stale = false;
    server().post<{ lines: GapLine[]; others: GapLine[]; names: string[]; cut?: boolean }>('api/files/' + props.rev + '/search',
      { q: asked, case: caseSensitive, all: !!whole, others: true, paths: paths }).then(function (res) {
      if (stale) return;
      setGaps(res.ok
        ? { for: asking, lines: res.lines, others: { lines: res.others || [], names: res.names || [] }, cut: !!res.cut }
        : { for: asking, lines: [], cut: false });
    });
    return function () { stale = true; };
  }, [asking, pathsKey]);
  var waiting = !!asking && (!gaps || gaps.for !== asking);
  // (While all of them are being found, what was found so far stays.)
  var last = useRef<{ for: string; result: SearchResult } | null>(null);
  var result: SearchResult = useMemo(function () {
    if (waiting && whole && last.current && last.current.for === wanted) return last.current.result;
    var have = gaps && gaps.for === asking ? gaps : null;
    var r = searchRevision(props.files, props.threads, query, have ? have.lines : undefined,
      whole ? { most: Infinity, until: whole.at + SEARCH_ALL_TIME } : undefined, have ? have.others : undefined);
    if (have && have.cut) { r.capped = true; if (whole) r.timedOut = true; }
    last.current = { for: wanted, result: r };
    return r;
  }, [props.files, props.threads, asked, caseSensitive, gaps, asking, waiting]);
  // Each hit's place in the order they are gone to.
  var order = useMemo(function () {
    var m = new Map<Hit, number>();
    result.all.forEach(function (h, i) { m.set(h, i); });
    return m;
  }, [result]);
  // How many the list draws (more as its end is scrolled to).
  var _r = useState(ROWS);
  var rows = _r[0];
  var setRows = _r[1];
  useEffect(function () { setRows(ROWS); }, [result]);
  var end = useRef<HTMLDivElement | null>(null);
  useEffect(function () {
    var el = end.current;
    if (!el || typeof IntersectionObserver === 'undefined') return undefined;
    var watch = new IntersectionObserver(function (seen) {
      if (seen.some(function (e) { return e.isIntersecting; })) setRows(function (n) { return n + ROWS; });
    });
    watch.observe(el);
    return function () { watch.disconnect(); };
  });
  // A new search starts before its first hit (Enter goes to it). (Not when
  // the same is found again: the one gone to stays the one.)
  useEffect(function () { searchCurrent.set(-1); }, [asked, caseSensitive, props.rev]);
  var total = result.all.length;
  var go = function (i: number) {
    if (!total) return;
    var at = ((i % total) + total) % total;
    searchCurrent.set(at);
    if (at >= rows) setRows(Math.ceil((at + 1) / ROWS) * ROWS);
    var hit = result.all[at];
    if (!links) return;
    // As a link goes: the address says where, and back returns from it. A
    // line the diff leaves out is shown first.
    // A file the page doesn't show is opened (as far as the line) first.
    if (hit.kind === 'line') {
      var line = hit;
      (line.gap ? reveal(props.rev, line.path, line.line)
        : line.other && opened ? opened.reach(props.rev, line.path, line.line)
        : Promise.resolve()).then(function () {
        links!.go({ kind: 'lines', path: line.path, side: line.side, start: line.line, end: line.line });
      });
    }
    else if (hit.kind === 'name') {
      var name = hit;
      (name.other && opened ? opened.reach(props.rev, name.path, 0) : Promise.resolve()).then(function () {
        links!.go({ kind: 'file', path: name.path });
      });
    }
    else links.go({ kind: 'thread', id: hit.thread });
  };
  var next = function () { go(current + 1); };
  var prev = function () { go(current < 0 ? total - 1 : current - 1); };
  // F3 and Shift+F3 too, wherever the keys are, while the search is shown.
  useEffect(function () {
    var key = function (e: KeyboardEvent) {
      if (e.key !== 'F3') return;
      e.preventDefault();
      if (e.shiftKey) prev(); else next();
    };
    document.addEventListener('keydown', key);
    return function () { document.removeEventListener('keydown', key); };
  });
  // What was found, marked in the page as far as it is drawn.
  usePaint(props.rev, query, total ? result.all[current] || null : null);
  var index = function (hit: Hit) { var i = order.get(hit); return i == null ? -1 : i; };
  var item = function (hit: Hit, children: preact.ComponentChildren, extra?: string) {
    var i = index(hit);
    return <button type="button" class={'diffnote-search__hit' + (i === current ? ' is-current' : '') + (extra ? ' ' + extra : '')} data-diffnote-search-hit={i}
      aria-current={i === current ? 'true' : undefined} onClick={function () { go(i); }}>{children}</button>;
  };
  var found = function (t: string, span: Span, before: number, most: number) {
    var e = excerpt(t, span, before, most);
    return <span class="diffnote-search__text">{e.head}<mark>{e.found}</mark>{e.tail}</span>;
  };
  // One file's hits: its name (or its path, faint), then its lines.
  var group = function (g: FileGroup) {
    var count = g.lines.length + (g.name ? 1 : 0);
    var first = g.name ? index(g.name) : g.lines.length ? index(g.lines[0]) : 0;
    if (first >= rows) return null;
    return <details key={g.path} class="diffnote-search__group" open data-diffnote-search-file={g.path}>
      <summary title={g.path}>
        {/* The file's own name first, its directory faint after it (the
            whole path, marked, where that is what was found). */}
        {g.name ? item(g.name, found(g.path, g.name.span, 1000, 1000), 'diffnote-search__name')
          : <span class="diffnote-search__path">{lib.baseName(g.path)}<small>{dirOf(g.path)}</small></span>}
        <span class="diffnote-badge diffnote-search__badge">{count}</span>
      </summary>
      {g.lines.length > 0 && <ol>
        {g.lines.map(function (h, j) {
          if (index(h) >= rows) return null;
          return <li key={j}>{item(h, <>
            <span class={'diffnote-search__line' + (h.side === 'old' ? ' is-old' : '')} title={lib.m(h.side === 'old' ? 'ui.search.old_title' : 'ui.search.new_title')}>
              {(h.side === 'old' ? '−' : '') + h.line}</span>
            {found(h.text, h.span, 16, 80)}
          </>)}</li>;
        })}
      </ol>}
    </details>;
  };
  return <div class="diffnote-search" data-diffnote-search data-diffnote-search-asked={waiting ? undefined : asked}>
    <div class="diffnote-search__box">
      <input ref={box} type="search" class="diffnote-search__input" data-diffnote-search-input value={text}
        placeholder={lib.m('ui.search.placeholder')} aria-label={lib.m('ui.search.placeholder')}
        onInput={function (e) { searchText.set(e.currentTarget.value); }}
        onKeyDown={function (e) {
          if (e.key === 'Enter') {
            e.preventDefault();
            // (What was typed is searched for first, if it hasn't been yet.)
            if (asked !== text) { setAsked(text); return; }
            if (e.shiftKey) prev(); else next();
          } else if (e.key === 'Escape') {
            e.preventDefault();
            showFiles();
          }
        }} />
      <button type="button" class={'diffnote-search__case' + (caseSensitive ? ' is-on' : '')} data-diffnote-search-case aria-pressed={caseSensitive}
        title={lib.m('ui.search.case_title')} onClick={function () { searchCase.set(!caseSensitive); }}>Aa</button>
    </div>
    <div class="diffnote-search__bar">
      <span class="diffnote-search__count" data-diffnote-search-count>
        {!asked ? ''
          : waiting && !total ? '…'
          : !total ? lib.m('ui.search.none')
          : current >= 0 ? lib.mf('ui.search.count_at', { at: String(current + 1), n: String(total) + (result.capped ? '+' : '') })
          : lib.mf('ui.search.count', { n: String(total) + (result.capped ? '+' : '') })}
      </span>
      <button type="button" class="diffnote-icon-button" data-diffnote-search-prev disabled={!total} title={lib.m('ui.search.prev_title')} aria-label={lib.m('ui.search.prev_title')}
        onClick={prev}><Icon name="up" /></button>
      <button type="button" class="diffnote-icon-button" data-diffnote-search-next disabled={!total} title={lib.m('ui.search.next_title')} aria-label={lib.m('ui.search.next_title')}
        onClick={next}><Icon name="down" /></button>
    </div>
    {result.capped && <p class="diffnote-search__note" data-diffnote-search-capped>
      {result.timedOut ? lib.m('ui.search.timed_out')
        : whole ? '…'
        : <button type="button" class="diffnote-button diffnote-search__all" data-diffnote-search-all
          onClick={function () { setWhole({ for: wanted, at: Date.now() }); }}>{lib.m('ui.search.all')}</button>}
    </p>}
    <div class="diffnote-search__results">
      {result.files.map(group)}
      {result.others.length > 0 && index(result.others[0].name || result.others[0].lines[0]) < rows && <p class="diffnote-search__section" data-diffnote-search-others>{lib.m('ui.search.others')}</p>}
      {result.others.map(group)}
      {result.comments.length > 0 && index(result.comments[0]) < rows && <details class="diffnote-search__group" open data-diffnote-search-comments>
        <summary><span class="diffnote-search__path">{lib.m('ui.search.comments')}</span><span class="diffnote-badge diffnote-search__badge">{result.comments.length}</span></summary>
        <ol>
          {result.comments.map(function (h, j) {
            if (index(h) >= rows) return null;
            return <li key={j}>{item(h, <>
              <span class="diffnote-search__who">{h.author}<small>{placeOf(props.placements[h.thread]) || lib.m('ui.search.review_wide')}</small></span>
              {found(h.text, h.span, 16, 80)}
            </>)}</li>;
          })}
        </ol>
      </details>}
      {total > rows && <div ref={end} class="diffnote-search__more" data-diffnote-search-more>…</div>}
    </div>
  </div>;
}

/** The directory a path is in (empty at the top). */
function dirOf(path: string): string {
  var i = path.lastIndexOf('/');
  return i < 0 ? '' : path.slice(0, i);
}

/** The text nodes under `el`, and where each starts in its text. */
function texts(el: Element): { node: Text; at: number }[] {
  var out: { node: Text; at: number }[] = [];
  var at = 0;
  var walk = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  for (var n = walk.nextNode(); n; n = walk.nextNode()) {
    out.push({ node: n as Text, at: at });
    at += (n as Text).data.length;
  }
  return out;
}

/** The part `[start, end)` of `el`'s text, as a range of the page. */
function rangeOf(nodes: { node: Text; at: number }[], start: number, end: number): Range | null {
  var range = document.createRange();
  var from = false;
  for (var i = 0; i < nodes.length; i++) {
    var n = nodes[i];
    var len = n.node.data.length;
    if (!from && start < n.at + len) { range.setStart(n.node, start - n.at); from = true; }
    if (from && end <= n.at + len) { range.setEnd(n.node, end - n.at); return range; }
  }
  return null;
}

/** Where in the page what a hit names is drawn: the text it is in. */
function placeInPage(section: Element, hit: Hit): Element | null {
  if (hit.kind === 'comment') return section.querySelector('article[data-diffnote-comment="' + CSS.escape(hit.comment) + '"] .diffnote-comment__body');
  var file = Array.prototype.filter.call(section.querySelectorAll('section.diffnote-file'), function (s: Element) {
    return s.getAttribute('data-diffnote-file') === hit.path;
  })[0] as Element | undefined;
  if (!file) return null;
  if (hit.kind === 'name') return file.querySelector('h2');
  var at = file.querySelector('[data-diffnote-' + hit.side + '="' + hit.line + '"]');
  if (!at) return null;
  // A row of one column carries its numbers; side by side, the number's cell
  // is just before the side's text.
  return at.tagName === 'TR' ? at.querySelector('.diffnote-line__content code') : at.nextElementSibling && at.nextElementSibling.querySelector('code');
}

/**
 * Marks what is searched for in the page (the diff's lines, the comments,
 * the files' names), and, more strongly, the hit gone to: with the page's
 * highlights, which leave what is drawn as it is. Again whenever the page
 * draws more (a file opened, lines shown). Where the browser has no
 * highlights, nothing is marked (the list still goes to each).
 */
function usePaint(rev: number, query: SearchQuery, now: Hit | null) {
  var highlights = typeof CSS !== 'undefined' ? (CSS as unknown as { highlights?: Map<string, unknown> }).highlights : undefined;
  useEffect(function () {
    const map = highlights;
    if (!map) return undefined;
    var Highlight = (window as unknown as { Highlight?: new (...r: Range[]) => { add(r: Range): void } }).Highlight;
    if (!Highlight) return undefined;
    var section = document.getElementById('rev-' + rev);
    var paint = function () {
      var all = new Highlight!();
      var here = new Highlight!();
      if (section && query.text) {
        section.querySelectorAll('.diffnote-line__content code, .diffnote-comment__body, section.diffnote-file h2').forEach(function (el) {
          var nodes = texts(el);
          occurrences(el.textContent || '', query).forEach(function (s) {
            var r = rangeOf(nodes, s[0], s[1]);
            if (r) all.add(r);
          });
        });
        var el = now && placeInPage(section, now);
        if (el && now) {
          var spans = occurrences(el.textContent || '', query);
          var s = spans[now.kind === 'name' ? 0 : Math.min(now.nth, spans.length - 1)];
          var r = s && rangeOf(texts(el), s[0], s[1]);
          if (r) here.add(r);
        }
      }
      map.set('diffnote-search', all);
      map.set('diffnote-search-now', here);
    };
    var frame = 0;
    var again = function () {
      if (frame) return;
      frame = requestAnimationFrame(function () { frame = 0; paint(); });
    };
    paint();
    var watch = new MutationObserver(again);
    if (section) watch.observe(section, { childList: true, subtree: true, characterData: true });
    return function () {
      watch.disconnect();
      if (frame) cancelAnimationFrame(frame);
      map.delete('diffnote-search');
      map.delete('diffnote-search-now');
    };
  }, [rev, query.text, query.caseSensitive, now]);
}
