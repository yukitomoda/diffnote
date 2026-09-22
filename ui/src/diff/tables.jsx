// The diff itself: one table of lines, or two side by side.
import { useContext, useMemo, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { transport } from '../transport.ts';
import { tokens } from '../markdown.tsx';
import { ComposeContext } from '../state/contexts.ts';
import { Card } from '../thread/Card.jsx';
import { Composer } from '../thread/Composer.jsx';

// What stands for the lines a diff leaves out: buttons to show some of them
// (next to the hunk above, next to the hunk below) or all.
function Expander(props) {
  var m = props.marker;
  var _ = useState(false);
  var busy = _[0];
  var setBusy = _[1];
  var go = function (where) {
    if (busy) return;
    setBusy(true);
    props.expand(m.gap, where).then(function () { setBusy(false); });
  };
  var step = lib.EXPAND_STEP;
  // Can they be had? Carried by the page (an export), or asked of the server.
  var can = m.x && (m.embedded || !!transport);
  if (!can) {
    return <div class="diffnote-expand"><span class="diffnote-expand__label">{lib.mf('ui.expand.left_label', { n: String(m.left) })}</span>{m.x && !m.embedded && !transport && <span class="diffnote-expand__note">{lib.m('ui.expand.not_embedded_note')}</span>}</div>;
  }
  return <div class="diffnote-expand">
    {m.left > step && m.prev && <button type="button" class="diffnote-expand__button" data-diffnote-expand="top" disabled={busy} onClick={function () { go('top'); }}>{lib.mf('ui.expand.up_button', { n: String(step) })}</button>}
    {m.left > step && m.next && <button type="button" class="diffnote-expand__button" data-diffnote-expand="bottom" disabled={busy} onClick={function () { go('bottom'); }}>{lib.mf('ui.expand.down_button', { n: String(step) })}</button>}
    <button type="button" class="diffnote-expand__button diffnote-expand__all" data-diffnote-expand="all" disabled={busy} onClick={function () { go('all'); }}>{lib.mf('ui.expand.all_button', { n: String(m.left) })}</button>
  </div>;
}

// The rows of one file's diff, with the cards of the threads on them.
export function DiffTable(props) {
  var file = props.file;
  var ctx = props.ctx;
  var cover = useMemo(
    function () {
      return lib.coverage(ctx.order, ctx.placements, file.path);
    },
    [ctx.order, ctx.placements, file.path]
  );
  var after = useMemo(
    function () {
      return lib.cardsAfter(ctx.order, ctx.placements, file.path);
    },
    [ctx.order, ctx.placements, file.path]
  );
  var compose = useContext(ComposeContext);
  var flat = useMemo(function () { return lib.flatRows(file); }, [file]);
  var sel = compose && compose.sel && compose.sel.rev === ctx.rev && compose.sel.path === file.path ? compose.sel : null;
  var lo = sel ? Math.min(sel.anchor, sel.to) : -1;
  var hi_ = sel ? Math.max(sel.anchor, sel.to) : -1;
  var flatIndex = 0;
  var out = [];
  file.hunks.forEach(function (hunk, hi) {
    if (hunk.marker) {
      out.push(<tr class="diffnote-expand-row" key={'g' + hi}><td colspan="3"><Expander marker={hunk.marker} expand={props.expand} /></td></tr>);
      return;
    }
    if (!file.opened && !hunk.quiet) out.push(<tr class="diffnote-hunk-header" key={'h' + hi}><td colspan="3">{hunk.header}</td></tr>);
    hunk.rows.forEach(function (row, ri) {
      var idx = flatIndex++;
      var picked = idx >= lo && idx <= hi_;
      var ids = lib.covering(cover, row);
      var resolvedOnly = ctx.hideResolved && ids.length > 0 && ids.every(function (id) { return ctx.byId[id].resolved; });
      var cls =
        'diffnote-line--' + (row.k === 'c' ? 'context' : row.k === 'a' ? 'added' : 'removed') +
        (ids.length ? ' diffnote-line--commented' : '') +
        (resolvedOnly ? ' diffnote-line--resolved-only' : '') +
        (picked ? ' diffnote-select' + (idx === lo ? ' diffnote-select-first' : '') + (idx === hi_ ? ' diffnote-select-last' : '') : '');
      // The bars are of the threads that are shown (not those hidden as resolved).
      var colors = lib.shownIds(ids, ctx.byId, ctx.hideResolved).map(function (id) { return ctx.placements[id].color; });
      var begin = compose && function (e) {
        if (e.button !== 0) return;
        // Against another revision, a removed line is not in this one.
        if (ctx.compare && row.k === 'd') return;
        e.preventDefault();
        compose.begin(ctx.rev, file.path, idx, e.shiftKey);
      };
      out.push(<tr
        class={cls}
        key={hi + ':' + ri}
        data-diffnote-old={row.o != null ? row.o : undefined}
        data-diffnote-new={row.n != null ? row.n : undefined}
        onMouseOver={compose ? function () { compose.extend(idx); } : undefined}
        data-diffnote-threads={ids.length ? ids.join(' ') : undefined}
        style={ids.length ? '--diffnote-bars: ' + lib.bars(colors) : undefined}
      >
        <td class="diffnote-line__gutter-old" onMouseDown={begin}>{row.o != null ? row.o : ''}</td>
        <td class="diffnote-line__gutter-new" onMouseDown={begin}>{row.n != null ? row.n : ''}</td>
        <td class="diffnote-line__content"><code>{tokens(row.t, row.w)}</code></td>
      </tr>);
      if (sel && !compose.selecting && idx === hi_) {
        var c = lib.counters(flat, sel.anchor, sel.to);
        out.push(<tr class="diffnote-composer-row" key="compose"><td colspan="3">
          <Composer scope="lines" where={lib.chosenLocation(file.path, c)} copy={lib.chosenLocation(file.path, c) + '@' + (ctx.rev + 1)}
            request={ctx.compare ? { revision: ctx.rev, file: file.path, head: c.head } : { revision: ctx.rev, file: file.path, base: c.base, head: c.head }} />
        </td></tr>);
      }
      lib.cardsOfRow(after, row).forEach(function (id) {
        out.push(<tr class="diffnote-thread-row" key={'c' + id}><td colspan="3"><Card rev={ctx.rev} thread={ctx.byId[id]} placement={ctx.placements[id]} /></td></tr>);
      });
    });
  });
  return <div class="diffnote-diff-scroll"><table class="diffnote-diff" data-diffnote-file={file.path}><tbody>{out}</tbody></table></div>;
}

// The same rows side by side: what a file was on the left, what it is on the
// right. A run of removed rows sits beside the run of added rows after it.
// A thread's mark (its color bar) is on the gutter of the side it is on; its
// range, when hovered, is shown on the whole row.
export function SplitTable(props) {
  var file = props.file;
  var ctx = props.ctx;
  var cover = useMemo(
    function () {
      return lib.coverage(ctx.order, ctx.placements, file.path);
    },
    [ctx.order, ctx.placements, file.path]
  );
  var after = useMemo(
    function () {
      return lib.cardsAfter(ctx.order, ctx.placements, file.path);
    },
    [ctx.order, ctx.placements, file.path]
  );
  var hidden = function (ids) {
    return ctx.hideResolved && ids.length > 0 && ids.every(function (id) { return ctx.byId[id].resolved; });
  };
  // Lines are chosen on one side: the cells of that side, from the first row
  // chosen to the last.
  var compose = useContext(ComposeContext);
  var flat = useMemo(function () { return lib.flatRows(file); }, [file]);
  var indexOf = useMemo(function () {
    var m = new Map();
    flat.forEach(function (f, i) { m.set(f.row, i); });
    return m;
  }, [flat]);
  var sel = compose && compose.sel && compose.sel.rev === ctx.rev && compose.sel.path === file.path && compose.sel.side ? compose.sel : null;
  var lo = sel ? Math.min(sel.anchor, sel.to) : -1;
  var hi_ = sel ? Math.max(sel.anchor, sel.to) : -1;
  var idxOf = function (row) { return row ? indexOf.get(row) : undefined; };
  var pickedCell = function (row, side) {
    if (!sel || sel.side !== side || !row) return '';
    var i = idxOf(row);
    var has = side === 'old' ? row.o != null : row.n != null;
    if (!has || i < lo || i > hi_) return '';
    return ' is-picked' + (i === lo ? ' is-picked-first' : '') + (i === hi_ ? ' is-picked-last' : '');
  };
  var begin = function (row, side) {
    // (Against another revision, only the new side is this revision's.)
    if (ctx.compare && side === 'old') return undefined;
    return compose && row && function (e) {
      if (e.button !== 0) return;
      e.preventDefault();
      compose.begin(ctx.rev, file.path, idxOf(row), e.shiftKey, side);
    };
  };
  var cellKind = function (row, side) {
    if (!row) return 'empty';
    return row.k === 'c' ? 'context' : side === 'old' ? 'removed' : 'added';
  };
  var out = [];
  file.hunks.forEach(function (hunk, hi) {
    if (hunk.marker) {
      out.push(<tr class="diffnote-expand-row" key={'g' + hi}><td colspan="4"><Expander marker={hunk.marker} expand={props.expand} /></td></tr>);
      return;
    }
    if (!hunk.quiet) out.push(<tr class="diffnote-hunk-header" key={'h' + hi}><td colspan="4">{hunk.header}</td></tr>);
    lib.pairRows(hunk.rows).forEach(function (pair, pi) {
      var l = pair.left;
      var r = pair.right;
      var idsL = l && l.o != null ? cover.old[l.o] || [] : [];
      var idsR = r && r.n != null ? cover.new[r.n] || [] : [];
      var ids = idsL.concat(idsR.filter(function (id) { return idsL.indexOf(id) < 0; }));
      var shownL = idsL.length > 0 && !hidden(idsL);
      var shownR = idsR.length > 0 && !hidden(idsR);
      var bars = function (side) {
        return '--diffnote-bars: ' + lib.bars(lib.shownIds(side, ctx.byId, ctx.hideResolved).map(function (id) { return ctx.placements[id].color; }));
      };
      var kl = cellKind(l, 'old');
      var kr = cellKind(r, 'new');
      var pl = pickedCell(l, 'old');
      var pr = pickedCell(r, 'new');
      out.push(<tr class="diffnote-split-row" key={hi + ':' + pi} data-diffnote-threads={ids.length ? ids.join(' ') : undefined}
        onMouseOver={compose ? function () { compose.extend(function (side) { return side === 'old' ? idxOf(l) : idxOf(r); }); } : undefined}>
        <td class={'diffnote-line__gutter-old diffnote-cell--' + kl + (shownL ? ' diffnote-gutter--commented' : '') + pl} style={shownL ? bars(idsL) : undefined}
          data-diffnote-old={l && l.o != null ? l.o : undefined} onMouseDown={begin(l, 'old')}>{l && l.o != null ? l.o : ''}</td>
        <td class={'diffnote-line__content diffnote-cell--' + kl + pl}>{l && <code>{tokens(l.t, l.w)}</code>}</td>
        <td class={'diffnote-line__gutter-new diffnote-cell--' + kr + (shownR ? ' diffnote-gutter--commented' : '') + pr} style={shownR ? bars(idsR) : undefined}
          data-diffnote-new={r && r.n != null ? r.n : undefined} onMouseDown={begin(r, 'new')}>{r && r.n != null ? r.n : ''}</td>
        <td class={'diffnote-line__content diffnote-cell--' + kr + pr}>{r && <code>{tokens(r.t, r.w)}</code>}</td>
      </tr>);
      // The box for the choice: under the pair that has its last row.
      var last = sel && !compose.selecting ? flat[hi_].row : null;
      if (last && (l === last || r === last)) {
        var c = lib.counters(flat, sel.anchor, sel.to, sel.side);
        out.push(<tr class="diffnote-composer-row" key="compose"><td colspan="4">
          <Composer scope="lines" where={lib.chosenLocation(file.path, c)} copy={lib.chosenLocation(file.path, c) + '@' + (ctx.rev + 1)}
            request={ctx.compare ? { revision: ctx.rev, file: file.path, head: c.head } : { revision: ctx.rev, file: file.path, base: c.base, head: c.head }} />
        </td></tr>);
      }
      // The cards of the pair: those of its new-side line, then its old-side line.
      var cards = lib.cardsOfRow(after, r || {});
      if (l && l !== r) cards = cards.concat(lib.cardsOfRow(after, { o: l.o }));
      cards.forEach(function (id) {
        out.push(<tr class="diffnote-thread-row" key={'c' + id}><td colspan="4"><Card rev={ctx.rev} thread={ctx.byId[id]} placement={ctx.placements[id]} /></td></tr>);
      });
    });
  });
  return <div class="diffnote-diff-scroll"><table class="diffnote-diff diffnote-diff--split" data-diffnote-file={file.path}>
    <colgroup><col class="diffnote-col-gutter" /><col /><col class="diffnote-col-gutter" /><col /></colgroup>
    <tbody>{out}</tbody>
  </table></div>;
}
