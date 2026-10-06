// The search, in the left pane in place of the files and threads: a box,
// how many were found and which is the one gone to, and what was found,
// file by file, then the comments. Pressing one goes to it (a folded file
// is opened, one marked as looked at brought back); Enter and Shift+Enter go
// to the next and the one before. What was found is marked in the page
// itself as well, as far as the page has drawn it (`paint`).
import { useContext, useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { useStore } from '@nanostores/preact';
import { lib } from '../lib.ts';
import { LinksContext } from '../state/contexts.ts';
import { searchCase, searchCurrent, searchFocus, searchText, showFiles } from '../state/search.ts';
import { excerpt, occurrences, placeOf, searchRevision } from '../search.ts';
import type { Hit, SearchQuery, SearchResult, Span } from '../search.ts';
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
var SETTLE = 150;

export function SearchPane(props: SearchPaneProps) {
  var links = useContext(LinksContext);
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
  var result: SearchResult = useMemo(function () {
    return searchRevision(props.files, props.threads, query);
  }, [props.files, props.threads, asked, caseSensitive]);
  // A new search starts before its first hit (Enter goes to it).
  useEffect(function () { searchCurrent.set(-1); }, [result]);
  var total = result.all.length;
  var go = function (i: number) {
    if (!total) return;
    var at = ((i % total) + total) % total;
    searchCurrent.set(at);
    var hit = result.all[at];
    if (!links) return;
    if (hit.kind === 'line') links.jump(props.rev, { kind: 'lines', path: hit.path, side: hit.side, start: hit.line, end: hit.line });
    else if (hit.kind === 'name') links.jump(props.rev, { kind: 'file', path: hit.path });
    else links.jump(props.rev, { kind: 'thread', id: hit.thread });
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
  var index = function (hit: Hit) { return result.all.indexOf(hit); };
  var item = function (hit: Hit, children: preact.ComponentChildren, extra?: string) {
    var i = index(hit);
    return <button type="button" class={'diffnote-search__hit' + (i === current ? ' is-current' : '') + (extra ? ' ' + extra : '')} data-diffnote-search-hit={i}
      aria-current={i === current ? 'true' : undefined} onClick={function () { go(i); }}>{children}</button>;
  };
  var found = function (t: string, span: Span, before: number, most: number) {
    var e = excerpt(t, span, before, most);
    return <span class="diffnote-search__text">{e.head}<mark>{e.found}</mark>{e.tail}</span>;
  };
  return <div class="diffnote-search" data-diffnote-search data-diffnote-search-asked={asked}>
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
        {!asked ? lib.m('ui.search.hint')
          : !total ? lib.m('ui.search.none')
          : current >= 0 ? lib.mf('ui.search.count_at', { at: String(current + 1), n: String(total) + (result.capped ? '+' : '') })
          : lib.mf('ui.search.count', { n: String(total) + (result.capped ? '+' : '') })}
      </span>
      <button type="button" class="diffnote-icon-button" data-diffnote-search-prev disabled={!total} title={lib.m('ui.search.prev_title')} aria-label={lib.m('ui.search.prev_title')}
        onClick={prev}><Icon name="up" /></button>
      <button type="button" class="diffnote-icon-button" data-diffnote-search-next disabled={!total} title={lib.m('ui.search.next_title')} aria-label={lib.m('ui.search.next_title')}
        onClick={next}><Icon name="down" /></button>
    </div>
    {result.capped && <p class="diffnote-search__note" data-diffnote-search-capped>{lib.mf('ui.search.capped', { n: String(total) })}</p>}
    <div class="diffnote-search__results">
      {result.files.map(function (g) {
        var count = g.lines.length + (g.name ? 1 : 0);
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
              return <li key={j}>{item(h, <>
                <span class={'diffnote-search__line' + (h.side === 'old' ? ' is-old' : '')} title={lib.m(h.side === 'old' ? 'ui.search.old_title' : 'ui.search.new_title')}>
                  {(h.side === 'old' ? '−' : '') + h.line}</span>
                {found(h.text, h.span, 16, 80)}
              </>)}</li>;
            })}
          </ol>}
        </details>;
      })}
      {result.comments.length > 0 && <details class="diffnote-search__group" open data-diffnote-search-comments>
        <summary><span class="diffnote-search__path">{lib.m('ui.search.comments')}</span><span class="diffnote-badge diffnote-search__badge">{result.comments.length}</span></summary>
        <ol>
          {result.comments.map(function (h, j) {
            return <li key={j}>{item(h, <>
              <span class="diffnote-search__who">{h.author}<small>{placeOf(props.placements[h.thread]) || lib.m('ui.search.review_wide')}</small></span>
              {found(h.text, h.span, 16, 80)}
            </>)}</li>;
          })}
        </ol>
      </details>}
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
