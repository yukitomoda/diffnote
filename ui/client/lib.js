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
      return p.file + ':' + (p.side === 'old' ? 'L' : '') + (p.end > p.start ? p.start + '-' + p.end : p.start);
    }
    return p.file;
  };

  // The same with only the file's name, for the narrow thread list.
  lib.shortLocation = function (p) {
    if (!p || p.kind === 'global') return '全体';
    var name = lib.baseName(p.file);
    if (p.kind === 'line') return name + ':' + (p.side === 'old' ? 'L' : '') + (p.end > p.start ? p.start + '-' + p.end : p.start);
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
      } else if (n.t === 'image') {
        out += n.alt || '[画像]';
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
  // How many lines a file's diff adds and removes (as the page shows it).
  lib.diffStat = function (file) {
    var added = 0;
    var removed = 0;
    (file.hunks || []).forEach(function (hunk) {
      (hunk.rows || []).forEach(function (row) {
        if (row.k === 'a') added++;
        else if (row.k === 'd') removed++;
      });
    });
    return { added: added, removed: removed };
  };

  // Five blocks for a file's change, in the proportion of added to removed lines
  // (`a`, `d`), or all neutral (`n`) if there is no change; each side that has
  // any lines has at least one block.
  lib.diffBlocks = function (added, removed) {
    var total = added + removed;
    if (total === 0) return ['n', 'n', 'n', 'n', 'n'];
    var green = Math.round((5 * added) / total);
    var red = 5 - green;
    if (added > 0 && green === 0) { green = 1; red = 4; }
    if (removed > 0 && red === 0) { red = 1; green = 4; }
    var blocks = [];
    for (var i = 0; i < green; i++) blocks.push('a');
    for (var j = 0; j < red; j++) blocks.push('d');
    return blocks;
  };

  // The text of a row's pieces.
  var rowText = function (row) {
    return row.t.map(function (p) { return typeof p === 'string' ? p : p[1]; }).join('');
  };

  // The file with the lines of its change that differ only in white space shown
  // as unchanged: in a run of removed lines followed by added ones, a removed
  // line and the added line in its place (in order, as the words are paired)
  // that read the same without white space become one unchanged row, with both
  // line numbers. Nothing is renumbered, so what refers to a line (a thread,
  // the left-out places) still does.
  lib.withoutSpaceChanges = function (file) {
    var bare = function (row) { return rowText(row).replace(/\s+/g, ''); };
    var changed = false;
    var hunks = file.hunks.map(function (hunk) {
      var rows = [];
      var i = 0;
      while (i < hunk.rows.length) {
        if (hunk.rows[i].k !== 'd') { rows.push(hunk.rows[i]); i++; continue; }
        var removed = [];
        var added = [];
        while (i < hunk.rows.length && hunk.rows[i].k === 'd') removed.push(hunk.rows[i++]);
        while (i < hunk.rows.length && hunk.rows[i].k === 'a') added.push(hunk.rows[i++]);
        var pendingRemoved = [];
        var pendingAdded = [];
        var flush = function () {
          pendingRemoved.forEach(function (r) { rows.push(r); });
          pendingAdded.forEach(function (r) { rows.push(r); });
          pendingRemoved = [];
          pendingAdded = [];
        };
        var pairs = Math.min(removed.length, added.length);
        for (var k = 0; k < pairs; k++) {
          if (bare(removed[k]) === bare(added[k])) {
            flush();
            rows.push({ k: 'c', o: removed[k].o, n: added[k].n, t: added[k].t });
            changed = true;
          } else {
            pendingRemoved.push(removed[k]);
            pendingAdded.push(added[k]);
          }
        }
        removed.slice(pairs).forEach(function (r) { pendingRemoved.push(r); });
        added.slice(pairs).forEach(function (r) { pendingAdded.push(r); });
        flush();
      }
      return Object.assign({}, hunk, { rows: rows });
    });
    return changed ? Object.assign({}, file, { hunks: hunks }) : file;
  };

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
    var old = counters.head.len === 0;
    var part = old ? counters.base : counters.head;
    var end = part.start + part.len - 1;
    return path + ':' + (old ? 'L' : '') + (end > part.start ? part.start + '-' + end : part.start);
  };

  // The places in a text that say lines of a file of the review: `a/b.ts:10`,
  // `a/b.ts:10-20`, with `L` for the old side (a line that was removed) and `R`
  // (or nothing) for the new, and `@2` after it for the revision (as the tabs
  // number them): the text as pieces, a piece being a string, or
  // `{ text, path, side, start, end, rev }` (`rev` is `null` if none is named)
  // for such a place. `has(path)` says whether a path is a file of the review
  // (a `12:30` or a `http://…:8080` is not a place), `revisions` how many
  // revisions there are.
  lib.lineRefs = function (text, has, revisions) {
    var pieces = [];
    var from = 0;
    var re = /:([LR])?(\d+)(?:-(\d+))?(?:@(\d+))?/g;
    var m;
    while ((m = re.exec(text))) {
      // The path is what comes before, up to a space: the longest end of it that
      // is a file (it may have something in front, such as a quote).
      var tokenStart = m.index;
      while (tokenStart > from && !/\s/.test(text.charAt(tokenStart - 1))) tokenStart--;
      var path = null;
      var pathStart = tokenStart;
      for (var i = tokenStart; i < m.index; i++) {
        var candidate = text.slice(i, m.index);
        if (has(candidate)) { path = candidate; pathStart = i; break; }
      }
      if (path === null) continue;
      var start = +m[2];
      var end = m[3] != null ? +m[3] : start;
      if (start < 1 || end < start) continue;
      var rev = m[4] != null ? +m[4] : null;
      if (rev !== null && (rev < 1 || rev > revisions)) continue;
      if (pathStart > from) pieces.push(text.slice(from, pathStart));
      pieces.push({ text: text.slice(pathStart, m.index + m[0].length), path: path, side: m[1] === 'L' ? 'old' : 'new', start: start, end: end, rev: rev });
      from = m.index + m[0].length;
    }
    if (from < text.length) pieces.push(text.slice(from));
    return pieces;
  };

  // A size in bytes, roughly: `830 KB`, `2.4 MB`.
  lib.formatSize = function (bytes) {
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1024 * 1024) return Math.round(bytes / 1024) + ' KB';
    return (Math.round((bytes / (1024 * 1024)) * 10) / 10) + ' MB';
  };

  // A limit in bytes as megabytes to show (`5`, `2.5`), and back: the number a
  // person typed as bytes, or `null` if it isn't a number.
  lib.bytesToMB = function (bytes) {
    return Math.round((bytes / (1024 * 1024)) * 1000) / 1000;
  };
  lib.mbToBytes = function (text) {
    var mb = Number(String(text).trim().replace(',', '.'));
    if (!isFinite(mb) || String(text).trim() === '' || mb <= 0) return null;
    return Math.round(mb * 1024 * 1024);
  };

  // A text as a quotation in a comment (Markdown): each line after `> `, a blank
  // line as a bare `>`; nothing for a text with only white space.
  lib.quoteMarkdown = function (text) {
    var lines = String(text).replace(/\r\n?/g, '\n').replace(/^\n+|\s+$/g, '').split('\n');
    if (lines.join('').trim() === '') return '';
    return lines.map(function (l) { return l.trim() === '' ? '>' : '> ' + l; }).join('\n');
  };

  // A box's text with a quotation put after what is there (a blank line between),
  // and a blank line after it, ready to be written under.
  lib.appendQuote = function (existing, text) {
    var quote = lib.quoteMarkdown(text);
    if (quote === '') return existing;
    var before = String(existing).replace(/\s+$/, '');
    return (before === '' ? '' : before + '\n\n') + quote + '\n\n';
  };

  // The emoji (`[emoji, code, words]`, see `emoji.js`) that a search asks for: by
  // the code (`bug`, `+1`, also as `:bug:`), a word (`バグ`), or the emoji
  // itself. Codes that begin with it come first. An empty search is all of them.
  lib.findEmoji = function (list, query) {
    var q = String(query).trim().toLowerCase().replace(/^:|:$/g, '');
    if (q === '') return list.slice();
    var starts = [];
    var rest = [];
    list.forEach(function (e) {
      if (e[1].toLowerCase().indexOf(q) === 0 || e[0] === q) starts.push(e);
      else if (e[1].toLowerCase().indexOf(q) >= 0 || e[2].toLowerCase().indexOf(q) >= 0) rest.push(e);
    });
    return starts.concat(rest);
  };

  // A text with each `:code:` that is the code of an emoji written as the emoji
  // (`:+1:` as 👍); what isn't one (`12:30:45`, `:nope:`) is left as it is.
  lib.withShortcodes = function (list, text) {
    if (String(text).indexOf(':') < 0) return text;
    var by = {};
    list.forEach(function (e) { by[e[1]] = e[0]; });
    return String(text).replace(/:([a-z0-9_+-]+):/g, function (whole, code) {
      return Object.prototype.hasOwnProperty.call(by, code) ? by[code] : whole;
    });
  };

  // What a comment says for an image of the review, to put in its text.
  lib.imageMarkdown = function (id) {
    return '![画像](diffnote-image:' + id + ')';
  };

  // What a comment says for another file of the review: a link to save it,
  // named by what the file is called (the brackets that would end the name early
  // are left out).
  lib.fileMarkdown = function (name, id) {
    return '[' + name.replace(/[\[\]\\]/g, '') + '](diffnote-file:' + id + ')';
  };

  // The text with `insert` where the choice was (`from` to `to`), and where the
  // cursor goes after it.
  lib.insertAt = function (text, from, to, insert) {
    var start = Math.min(Math.max(from, 0), text.length);
    var end = Math.min(Math.max(to, start), text.length);
    return { text: text.slice(0, start) + insert + text.slice(end), cursor: start + insert.length };
  };

  if (typeof module !== 'undefined' && module.exports) module.exports = lib;
})(typeof window !== 'undefined' ? (window.Diffnote = window.Diffnote || {}) : (globalThis.Diffnote = globalThis.Diffnote || {}));
