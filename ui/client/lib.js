// Pure helpers of the page: nothing here touches the document, so they can be
// tested with Node (`node --test ui/client/test`). The page is a plain script
// (it is opened from a file: no modules), so this file adds to `Diffnote.lib`
// and, under Node, also exports it.
(function (D) {
  'use strict';
  var lib = (D.lib = D.lib || {});

  // The colors of threads (a placement carries the number of its color).
  lib.PALETTE = ['#1f77b4', '#ff7f0e', '#9467bd', '#8c564b', '#e377c2', '#17becf', '#bcbd22', '#7f7f7f'];

  lib.color = function (n) {
    return lib.PALETTE[n % lib.PALETTE.length];
  };

  // The last part of a path.
  lib.baseName = function (path) {
    var i = path.lastIndexOf('/');
    return i < 0 ? path : path.slice(i + 1);
  };

  // Where a thread is, as `path`, `path:LINE` or `path:FIRST-LAST` (the form
  // `diffnote edit --show` reads); null for a thread about the whole review.
  lib.location = function (p) {
    if (!p || p.kind === 'global') return null;
    if (p.kind === 'line') {
      return p.file + ':' + (p.end > p.start ? p.start + '-' + p.end : p.start);
    }
    return p.file;
  };

  // The same with only the file's name, for the narrow thread list.
  lib.shortLocation = function (p) {
    if (!p || p.kind === 'global') return '全体';
    var name = lib.baseName(p.file);
    if (p.kind === 'line') return name + ':' + (p.end > p.start ? p.start + '-' + p.end : p.start);
    return name;
  };

  // Of the threads on a line, those that are shown: not the resolved ones while
  // they are hidden. What is drawn for a line (its bars) is of these only.
  lib.shownIds = function (ids, byId, hideResolved) {
    return hideResolved
      ? ids.filter(function (id) {
          return !byId[id].resolved;
        })
      : ids;
  };

  // A line's pieces (text, or `[kind, text]`) cut where the words that changed
  // begin and end (`ranges`: `[start, end)` in UTF-16 units, sorted), as
  // `[kind or null, text, changed]`. What isn't in a range is not changed.
  lib.markPieces = function (pieces, ranges) {
    var out = [];
    var at = 0;
    var r = 0;
    (pieces || []).forEach(function (p) {
      var kind = typeof p === 'string' ? null : p[0];
      var text = typeof p === 'string' ? p : p[1];
      var start = 0;
      while (start < text.length) {
        var pos = at + start;
        while (r < (ranges || []).length && ranges[r][1] <= pos) r++;
        var range = ranges && r < ranges.length ? ranges[r] : null;
        var inside = range !== null && range[0] <= pos;
        // Up to where the state changes: the end of this range or the start of the next.
        var stop = inside ? range[1] : range ? range[0] : Infinity;
        var end = Math.min(text.length, stop - at);
        out.push([kind, text.slice(start, end), inside]);
        start = end;
      }
      at += text.length;
    });
    return out;
  };

  // The stacked color bars at a line's left edge, one per thread on it.
  lib.bars = function (colors) {
    return colors
      .map(function (c, i) {
        return 'inset ' + (3 + i * 4) + 'px 0 0 0 ' + lib.color(c);
      })
      .join(', ');
  };

  // Which threads are on which lines of one file: `{ new: {line: [ids]}, old: {...} }`,
  // from the placements of a revision. Threads are taken in the order given.
  lib.coverage = function (threadIds, placements, file) {
    var cover = { new: {}, old: {} };
    var add = function (side, a, b, id) {
      for (var n = a; n <= b; n++) {
        (cover[side][n] = cover[side][n] || []).push(id);
      }
    };
    threadIds.forEach(function (id) {
      var p = placements[id];
      if (!p || p.kind !== 'line' || p.file !== file) return;
      add(p.side, p.start, p.end, id);
      if (p.side === 'new' && p.old_range) add('old', p.old_range[0], p.old_range[1], id);
    });
    return cover;
  };

  // The threads that cover a row (a row has its line number on either side or
  // both), each once in a row.
  lib.covering = function (cover, row) {
    var ids = [];
    if (row.n != null && cover.new[row.n]) ids = ids.concat(cover.new[row.n]);
    if (row.o != null && cover.old[row.o]) ids = ids.concat(cover.old[row.o]);
    return ids.filter(function (id, i) {
      return ids.indexOf(id) === i;
    });
  };

  // Where the cards of a file's threads go: after the row that has a line
  // number on a side (`new:LINE` / `old:LINE`), the ids in the order of the
  // threads. A thread on lines goes after its last line; one whose lines are
  // not here, after the line before the point where they are.
  lib.cardsAfter = function (threadIds, placements, file) {
    var after = {};
    var put = function (key, id) {
      (after[key] = after[key] || []).push(id);
    };
    threadIds.forEach(function (id) {
      var p = placements[id];
      if (!p || p.file !== file) return;
      if (p.kind === 'line') put(p.side + ':' + p.end, id);
      else if (p.kind === 'point') put('new:' + Math.max(p.before - 1, 1), id);
    });
    return after;
  };

  // A row's cards: those after its new-side line, then those after its
  // old-side line.
  lib.cardsOfRow = function (after, row) {
    var ids = [];
    if (row.n != null && after['new:' + row.n]) ids = ids.concat(after['new:' + row.n]);
    if (row.o != null && after['old:' + row.o]) ids = ids.concat(after['old:' + row.o]);
    return ids;
  };

  // The rows of a hunk side by side: an unchanged row is on both sides; a run
  // of removed rows is put beside the run of added rows that follows it, top
  // to bottom, and the shorter side is left empty.
  lib.pairRows = function (rows) {
    var out = [];
    var i = 0;
    while (i < rows.length) {
      var row = rows[i];
      if (row.k === 'c') {
        out.push({ left: row, right: row });
        i++;
        continue;
      }
      var removed = [];
      var added = [];
      while (i < rows.length && rows[i].k === 'd') removed.push(rows[i++]);
      while (i < rows.length && rows[i].k === 'a') added.push(rows[i++]);
      for (var j = 0; j < Math.max(removed.length, added.length); j++) {
        out.push({ left: removed[j] || null, right: added[j] || null });
      }
    }
    return out;
  };

  // The text of a comment's nodes (see `src/html/markdown.rs`), a line for each
  // block and each break.
  lib.plainText = function (nodes) {
    var out = '';
    (nodes || []).forEach(function (n) {
      if (typeof n === 'string') {
        out += n;
      } else if (n.t === 'br') {
        out += '\n';
      } else if (n.t === 'code' || n.t === 'pre') {
        out += (n.s || '') + (n.t === 'pre' ? '\n' : '');
      } else {
        out += lib.plainText(n.c);
        if (['p', 'h', 'li', 'quote', 'ul', 'ol', 'hr'].indexOf(n.t) >= 0) out += '\n';
      }
    });
    return out;
  };

  // The first line of a comment as plain text, short, to tell threads apart in
  // the list.
  lib.preview = function (nodes) {
    var line = '';
    lib.plainText(nodes).split('\n').some(function (l) {
      line = l.trim();
      return line !== '';
    });
    var chars = Array.from(line);
    return chars.length > 48 ? chars.slice(0, 48).join('') + '…' : line;
  };

  // A time as `YYYY-MM-DD HH:MM` in the viewer's time zone.
  lib.formatTime = function (iso) {
    var d = new Date(iso);
    if (isNaN(d.getTime())) return iso;
    var two = function (n) {
      return (n < 10 ? '0' : '') + n;
    };
    return d.getFullYear() + '-' + two(d.getMonth() + 1) + '-' + two(d.getDate()) + ' ' + two(d.getHours()) + ':' + two(d.getMinutes());
  };

  // The threads of a revision that are on a file (lines, the file, a point,
  // or listed with it because they can't be placed): for the count beside it.
  lib.threadsOfFile = function (threadIds, placements, file) {
    return threadIds.filter(function (id) {
      var p = placements[id];
      return p && p.kind !== 'global' && p.file === file;
    });
  };

  // How many threads there are, and how many are resolved.
  lib.counts = function (threads) {
    return {
      all: threads.length,
      resolved: threads.filter(function (t) {
        return t.resolved;
      }).length,
    };
  };

  // --- The lines a diff leaves out ---------------------------------------

  // How many lines a press shows at a time.
  lib.EXPAND_STEP = 20;

  // The file's hunks with what has been shown of the lines between them:
  // `shown[i]` is `{ top: rows, bottom: rows }` for the place `file.gaps[i]`
  // (before hunk i): `top` the lines next to the hunk before it, `bottom` those
  // next to the hunk after it. What is still left out is a marker (`{ marker }`
  // with no rows) between them; a hunk whose place is shown whole, or shown up to it,
  // gives up its `@@` row (`quiet`). The added blocks have headers, so the rows can be told
  // their line numbers as those of a hunk.
  lib.withGaps = function (file, shown) {
    var gaps = file.gaps;
    if (!gaps || gaps.length === 0) return file;
    var count = file.hunks.length;
    var block = function (rows) {
      var o = rows[0].o;
      var n = rows[0].n;
      return { header: '@@ -' + o + ',' + rows.length + ' +' + n + ',' + rows.length + ' @@', rows: rows, quiet: true };
    };
    var hunks = [];
    for (var i = 0; i <= count; i++) {
      var g = gaps[i];
      var whole = false;
      if (g) {
        var st = (shown && shown[i]) || { top: [], bottom: [] };
        var left = g.n - st.top.length - st.bottom.length;
        // The hunk's `@@` row is not needed once lines next to it are shown:
        // they run into it (and it would sit in the middle of the code).
        whole = left <= 0 || st.bottom.length > 0;
        if (st.top.length) hunks.push(block(st.top));
        if (left > 0) {
          hunks.push({ marker: { gap: i, left: left, n: g.n, prev: i > 0, next: i < count, x: !!g.x, embedded: !!g.t }, header: '', rows: [] });
        }
        if (st.bottom.length) hunks.push(block(st.bottom));
      }
      if (i < count) hunks.push(whole ? Object.assign({}, file.hunks[i], { quiet: true }) : file.hunks[i]);
    }
    return Object.assign({}, file, { hunks: hunks });
  };

  // What has been shown of each place of a file's `gaps`, from the lines shown
  // so far (`revealed`: the pieces of each line, by new line number): the lines
  // from the start of a place and those up to its end. Kept by line number, so
  // that it holds when the places change (a thread brought in context).
  lib.shownFrom = function (gaps, revealed) {
    var out = {};
    (gaps || []).forEach(function (g, i) {
      if (!g) return;
      var top = [];
      while (top.length < g.n && revealed[g.w + top.length]) {
        top.push({ k: 'c', o: g.o + top.length, n: g.w + top.length, t: revealed[g.w + top.length] });
      }
      var bottom = [];
      while (top.length + bottom.length < g.n && revealed[g.w + g.n - 1 - bottom.length]) {
        var at = g.n - 1 - bottom.length;
        bottom.unshift({ k: 'c', o: g.o + at, n: g.w + at, t: revealed[g.w + at] });
      }
      out[i] = { top: top, bottom: bottom };
    });
    return out;
  };

  // What a press of `where` ('top', 'bottom' or 'all') on place `g` (with what is
  // shown of it) asks for: `{ offset, count, side }` (the lines from `offset`
  // of the place, added to the `side`), or null if nothing is left.
  lib.expandRequest = function (g, st, where) {
    var left = g.n - st.top.length - st.bottom.length;
    if (left <= 0) return null;
    if (where === 'all') return { offset: st.top.length, count: left, side: 'top' };
    var count = Math.min(lib.EXPAND_STEP, left);
    if (where === 'top') return { offset: st.top.length, count: count, side: 'top' };
    return { offset: g.n - st.bottom.length - count, count: count, side: 'bottom' };
  };

  // The rows for lines of place `g`, from its `offset`, given the pieces of each.
  lib.gapRows = function (g, offset, lines) {
    return lines.map(function (pieces, i) {
      return { k: 'c', o: g.o + offset + i, n: g.w + offset + i, t: pieces };
    });
  };

  // --- Choosing lines to comment on --------------------------------------

  // All the rows of a file's diff in one list (across its hunks), each with
  // the line counters on each side as they stand before it: the old and the
  // new line number the next line would have.
  lib.flatRows = function (file) {
    var out = [];
    file.hunks.forEach(function (hunk, hi) {
      if (hunk.marker) return;
      var m = /^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(hunk.header);
      var o = m ? +m[1] : 1;
      var n = m ? +m[2] : 1;
      hunk.rows.forEach(function (row, ri) {
        out.push({ hi: hi, ri: ri, row: row, oldNext: o, newNext: n });
        if (row.o != null) o += 1;
        if (row.n != null) n += 1;
      });
    });
    return out;
  };

  // The lines the rows from `a` to `b` (either way round) cover, on each side:
  // where the counters stand before the first, and after the last.
  //
  // With a `side` ('old' or 'new': the choice was made on one side of a side by
  // side view) the choice is of that side's lines, and `a` and `b` are rows that
  // have one. The other side's lines are those of the same rows only if all of
  // them are unchanged; otherwise it is none, at the place where the first row
  // is.
  lib.counters = function (flat, a, b, side) {
    var lo = Math.min(a, b);
    var hi = Math.max(a, b);
    var first = flat[lo];
    var last = flat[hi];
    var span = function (next, has) {
      var start = first[next];
      return { start: start, len: last[next] + (has ? 1 : 0) - start };
    };
    var base = span('oldNext', last.row.o != null);
    var head = span('newNext', last.row.n != null);
    if (side) {
      var unchanged = flat.slice(lo, hi + 1).every(function (f) {
        return f.row.k === 'c';
      });
      if (!unchanged) {
        if (side === 'old') head = { start: first.newNext, len: 0 };
        else base = { start: first.oldNext, len: 0 };
      }
    }
    return { base: base, head: head };
  };

  // `src/a.rs:10-13`: the lines of the choice, as the new side has them (the old
  // side, for lines that were only removed).
  lib.chosenLocation = function (path, counters) {
    var part = counters.head.len > 0 ? counters.head : counters.base;
    var end = part.start + part.len - 1;
    return path + ':' + (end > part.start ? part.start + '-' + end : part.start);
  };

  if (typeof module !== 'undefined' && module.exports) module.exports = lib;
})(typeof window !== 'undefined' ? (window.Diffnote = window.Diffnote || {}) : (globalThis.Diffnote = globalThis.Diffnote || {}));
