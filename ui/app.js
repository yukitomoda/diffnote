(function () {
  var slice = Array.prototype.slice;
  var views = slice.call(document.querySelectorAll('.diffnote-revision'));
  var links = slice.call(document.querySelectorAll('[data-diffnote-revision-link]'));

  // --- Which comment's range is shown ------------------------------------
  // One at a time: the one under the mouse or focus, or the pinned one
  // (click a comment or one of its lines; click elsewhere or Esc to let go).
  var THREAD = '[data-diffnote-thread-id]';
  var LINE = 'tr[data-diffnote-threads]';
  var active = null;
  var pinned = null;
  // While lines are being chosen by dragging (served page): the choice is
  // what is shown, so no comment's range is.
  var dragging = false;

  function scopeOf(el) { return el.closest('.diffnote-revision') || document; }
  function cardOf(scope, id) { return scope.querySelector('[data-diffnote-thread-id="' + id + '"]'); }
  function rowsOf(scope, id) { return slice.call(scope.querySelectorAll('tr[data-diffnote-threads~="' + id + '"]')); }

  function clear() {
    if (!active) return;
    active.rows.forEach(function (r) {
      r.classList.remove('diffnote-range', 'diffnote-range-first', 'diffnote-range-last');
      r.style.removeProperty('--rc');
    });
    if (active.card) {
      active.card.classList.remove('diffnote-hover');
      active.card.style.removeProperty('--rc');
    }
    active = null;
  }

  function activate(scope, id) {
    if (active && active.id === id && active.scope === scope) return;
    clear();
    var rows = rowsOf(scope, id);
    var card = cardOf(scope, id);
    var color = (card && card.getAttribute('data-diffnote-color')) || '#0969da';
    rows.forEach(function (r, k) {
      r.classList.add('diffnote-range');
      r.style.setProperty('--rc', color);
      if (k === 0) r.classList.add('diffnote-range-first');
      if (k === rows.length - 1) r.classList.add('diffnote-range-last');
    });
    if (card) {
      card.classList.add('diffnote-hover');
      card.style.setProperty('--rc', color);
    }
    active = { id: id, scope: scope, rows: rows, card: card };
  }

  // A line can be in several ranges: take the smallest, the most specific.
  function pick(scope, el) {
    var own = el.getAttribute('data-diffnote-thread-id');
    if (own) return own;
    var ids = (el.getAttribute('data-diffnote-threads') || '').split(' ').filter(Boolean);
    var best = null, size = Infinity;
    ids.forEach(function (id) {
      var n = rowsOf(scope, id).length;
      if (n < size) { best = id; size = n; }
    });
    return best;
  }

  function target(node) {
    return node && node.closest ? node.closest(THREAD + ', ' + LINE) : null;
  }

  document.addEventListener('mouseover', function (e) {
    if (pinned || dragging) return;
    var el = target(e.target);
    if (!el) return;
    var id = pick(scopeOf(el), el);
    if (id) activate(scopeOf(el), id);
  });
  document.addEventListener('mouseout', function (e) {
    if (pinned) return;
    if (target(e.relatedTarget)) return;
    clear();
  });
  document.addEventListener('focusin', function (e) {
    if (pinned) return;
    var el = target(e.target);
    if (!el) return;
    var id = pick(scopeOf(el), el);
    if (id) activate(scopeOf(el), id);
  });
  // A thread in the list: open its card, bring it to the middle of the
  // screen and keep its range shown.
  function jump(link) {
    var card = document.getElementById(link.getAttribute('href').slice(1));
    if (!card) return;
    for (var n = card; n; n = n.parentElement) {
      if (n.tagName === 'DETAILS') n.open = true;
    }
    card.scrollIntoView({ block: 'center' });
    pinned = card.getAttribute('data-diffnote-thread-id');
    activate(scopeOf(card), pinned);
  }

  // Copy buttons (file paths, thread locations). Handled before anything else
  // sees the click, since they sit inside <summary> elements.
  function copyText(text, button) {
    function done() {
      var before = button.textContent;
      button.textContent = 'コピーしました';
      button.classList.add('is-done');
      setTimeout(function () {
        button.textContent = before;
        button.classList.remove('is-done');
      }, 1400);
    }
    function fallback() {
      var ta = document.createElement('textarea');
      ta.value = text;
      ta.setAttribute('readonly', '');
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.select();
      try { document.execCommand('copy'); done(); } catch (err) { /* nothing to do */ }
      document.body.removeChild(ta);
    }
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(done, fallback);
    } else {
      fallback();
    }
  }
  document.addEventListener('click', function (e) {
    var button = e.target.closest ? e.target.closest('[data-diffnote-copy]') : null;
    if (!button) return;
    e.preventDefault();
    e.stopPropagation();
    copyText(button.getAttribute('data-diffnote-copy'), button);
  }, true);

  document.addEventListener('click', function (e) {
    var link = e.target.closest ? e.target.closest('a[data-diffnote-jump]') : null;
    if (link) { e.preventDefault(); jump(link); return; }
    // A line number on the served page starts a selection, not a pin.
    if (document.body.hasAttribute('data-diffnote-api') && e.target.closest &&
        e.target.closest('.diffnote-line__gutter-old, .diffnote-line__gutter-new')) return;
    var el = target(e.target);
    if (!el) { pinned = null; clear(); return; }
    var scope = scopeOf(el);
    var id = pick(scope, el);
    if (!id) return;
    if (pinned === id) { pinned = null; return; }
    pinned = id;
    activate(scope, id);
  });
  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape') { pinned = null; clear(); }
  });

  // --- Hiding resolved threads -------------------------------------------
  // The box in the top bar: on unless the choice was turned off (kept between
  // visits, where the browser allows).
  // Cards and list entries are hidden by the style; a line's mark is too
  // when every thread on it is resolved, which is worked out here (and again
  // whenever the page changes).
  var hideBox = document.querySelector('[data-diffnote-hide-resolved]');
  function syncResolved() {
    var hide = !!(hideBox && hideBox.checked);
    document.body.classList.toggle('diffnote-hide-resolved', hide);
    slice.call(document.querySelectorAll('.diffnote-revision')).forEach(function (view) {
      var done = {};
      slice.call(view.querySelectorAll('.diffnote-thread--resolved[data-diffnote-thread-id]')).forEach(function (c) {
        done[c.getAttribute('data-diffnote-thread-id')] = true;
      });
      slice.call(view.querySelectorAll('tr[data-diffnote-threads]')).forEach(function (row) {
        var ids = row.getAttribute('data-diffnote-threads').split(' ').filter(Boolean);
        row.classList.toggle('diffnote-line--resolved-only', hide && ids.length > 0 && ids.every(function (id) { return done[id]; }));
      });
    });
    var count = document.querySelector('[data-diffnote-resolved-count]');
    if (count) {
      var shownView = document.querySelector('.diffnote-revision.is-current') || document;
      var n = shownView.querySelectorAll('.diffnote-threadlist .is-resolved').length;
      count.textContent = n ? '(' + n + ')' : '';
    }
  }
  if (hideBox) {
    hideBox.checked = true;
    try { hideBox.checked = localStorage.getItem('diffnote-hide-resolved') !== '0'; } catch (e) { /* not kept: the default */ }
    hideBox.addEventListener('change', function () {
      try { localStorage.setItem('diffnote-hide-resolved', hideBox.checked ? '1' : '0'); } catch (e) { /* not kept */ }
      syncResolved();
    });
  }

  // --- Revisions ---------------------------------------------------------
  // Set by the served page: draws a view again (see `refresh` below).
  var refreshView = null;
  function show(i) {
    pinned = null;
    clear();
    views.forEach(function (v, k) { v.classList.toggle('is-current', k === i); });
    links.forEach(function (a, k) { a.classList.toggle('is-current', k === i); });
    if (refreshView && views[i].hasAttribute('data-stale')) refreshView(i);
    syncResolved();
  }
  if (views.length > 1) {
    document.documentElement.classList.add('diffnote-js');
    var start = views.length - 1;
    var m = /^#rev-(\d+)$/.exec(location.hash);
    if (m && +m[1] < views.length) start = +m[1];
    show(start);
    links.forEach(function (a, k) {
      a.addEventListener('click', function (e) { e.preventDefault(); show(k); });
    });
  }

  // --- Times in the viewer's own time zone -------------------------------
  function two(n) { return (n < 10 ? '0' : '') + n; }
  slice.call(document.querySelectorAll('time[datetime]')).forEach(function (t) {
    var d = new Date(t.getAttribute('datetime'));
    if (isNaN(d.getTime())) return;
    t.textContent = d.getFullYear() + '-' + two(d.getMonth() + 1) + '-' + two(d.getDate()) +
      ' ' + two(d.getHours()) + ':' + two(d.getMinutes());
    t.title = d.toLocaleString();
  });

  // --- Changing threads from the served page ----------------------------
  // A change is sent to the server, which answers with the HTML of what
  // changed; that is swapped in, so the page is never reloaded. The change
  // also shows at once (a reply as a faded comment, a resolve as the new
  // state) and is corrected by the answer, or undone with a message if it
  // fails.
  if (document.body.hasAttribute('data-diffnote-api')) {
    var post = function (path, data) {
      return fetch(path, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Diffnote': '1' },
        credentials: 'same-origin',
        body: JSON.stringify(data || {})
      }).then(function (r) {
        return r.json().catch(function () { return { ok: false, error: '応答を読めませんでした' }; });
      }, function () {
        return { ok: false, error: 'サーバーに接続できませんでした' };
      });
    };
    var swap = function (el, html) {
      var tpl = document.createElement('template');
      tpl.innerHTML = html.trim();
      var fresh = tpl.content.firstElementChild;
      el.replaceWith(fresh);
      return fresh;
    };
    var apply = function (res) {
      var again = active && active.id === res.thread ? active.scope : null;
      if (again) clear();
      res.views.forEach(function (v) {
        var card = document.getElementById('r' + v.revision + '-thread-' + res.thread);
        if (card) swap(card, v.card);
        var section = document.getElementById('rev-' + v.revision);
        var link = section && section.querySelector('a[data-diffnote-jump=' + JSON.stringify(res.thread) + ']');
        if (link && link.parentElement) swap(link.parentElement, v.list_item);
      });
      slice.call(document.querySelectorAll('.diffnote-side .diffnote-badge[title]')).forEach(function (b) {
        b.textContent = res.open + ' / ' + res.all;
      });
      var summary = document.querySelector('.diffnote-summary p');
      if (summary) summary.textContent = 'スレッド ' + res.all + ' 件(解決済み ' + (res.all - res.open) + ' 件)';
      syncResolved();
      if (again) {
        var current = document.querySelector('.diffnote-revision.is-current') || document;
        activate(current, res.thread);
      }
    };
    var showError = function (where, message) {
      var old = where.querySelector('.diffnote-error');
      if (old) old.remove();
      var p = document.createElement('p');
      p.className = 'diffnote-error';
      p.textContent = message;
      where.appendChild(p);
    };
    var send = function (form) {
      var box = form.querySelector('textarea');
      var text = box.value.trim();
      if (!text || form.classList.contains('is-sending')) return;
      form.classList.add('is-sending');
      box.disabled = true;
      var pending = document.createElement('article');
      pending.className = 'diffnote-comment is-pending';
      var who = document.createElement('p');
      who.className = 'diffnote-comment__author';
      who.textContent = '保存中…';
      var what = document.createElement('div');
      what.className = 'diffnote-comment__body';
      what.textContent = text;
      pending.appendChild(who);
      pending.appendChild(what);
      var actions = form.closest('.diffnote-thread__actions');
      actions.parentNode.insertBefore(pending, actions);
      post('/api/threads/' + form.getAttribute('data-diffnote-thread') + '/replies', { body: text }).then(function (res) {
        pending.remove();
        form.classList.remove('is-sending');
        box.disabled = false;
        if (res.ok) {
          apply(res);
        } else {
          showError(form, res.error || '保存できませんでした');
          box.focus();
        }
      });
    };
    document.addEventListener('submit', function (e) {
      var form = e.target.closest ? e.target.closest('.diffnote-reply') : null;
      if (!form) return;
      e.preventDefault();
      send(form);
    });
    document.addEventListener('keydown', function (e) {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && e.target.closest && e.target.closest('.diffnote-reply')) {
        e.preventDefault();
        send(e.target.closest('.diffnote-reply'));
      }
    });
    document.addEventListener('click', function (e) {
      var quit = e.target.closest ? e.target.closest('[data-diffnote-shutdown]') : null;
      if (quit) {
        post('/api/shutdown').then(function () {
          document.body.innerHTML = '<p style="padding:24px;font:14px sans-serif">終了しました。このタブは閉じてかまいません。</p>';
        });
        return;
      }
      var button = e.target.closest ? e.target.closest('[data-diffnote-action]') : null;
      if (!button) return;
      var id = button.getAttribute('data-diffnote-thread');
      var action = button.getAttribute('data-diffnote-action');
      var card = button.closest('.diffnote-thread');
      var was = card.classList.contains('diffnote-thread--resolved');
      // At once: the state and the button show what will be.
      card.classList.toggle('diffnote-thread--resolved', action === 'resolve');
      button.disabled = true;
      post('/api/threads/' + id + '/' + action).then(function (res) {
        if (res.ok) {
          apply(res);
        } else {
          card.classList.toggle('diffnote-thread--resolved', was);
          button.disabled = false;
          showError(button.closest('.diffnote-thread__actions'), res.error || '保存できませんでした');
        }
      });
    });

    // --- A new thread on lines ---------------------------------------------
    // Press a line number, or drag over several (Shift+click extends the last
    // choice); a box opens under the last line. Sent as counters on each side:
    // where they stand before the first line, and after the last.
    var sel = null;       // { table, rows, from } while lines are chosen
    var composer = null;  // the open box's <tr>

    var diffRows = function (table) {
      return slice.call(table.querySelectorAll('tr[data-diffnote-old-next]'));
    };
    var rowOf = function (node) {
      var td = node.closest ? node.closest('.diffnote-line__gutter-old, .diffnote-line__gutter-new') : null;
      var tr = td && td.closest('tr[data-diffnote-old-next]');
      return tr && tr.closest('table[data-diffnote-file]') ? tr : null;
    };
    var num = function (tr, name) { return +tr.getAttribute('data-diffnote-' + name); };
    var has = function (tr, name) { return tr.getAttribute('data-diffnote-' + name) !== ''; };

    var unchoose = function () {
      if (sel) sel.rows.forEach(function (r) {
        r.classList.remove('diffnote-select', 'diffnote-select-first', 'diffnote-select-last');
      });
    };
    var choose = function (table, from, to) {
      unchoose();
      var rows = diffRows(table);
      var a = rows.indexOf(from), b = rows.indexOf(to);
      var picked = rows.slice(Math.min(a, b), Math.max(a, b) + 1);
      picked.forEach(function (r) { r.classList.add('diffnote-select'); });
      picked[0].classList.add('diffnote-select-first');
      picked[picked.length - 1].classList.add('diffnote-select-last');
      sel = { table: table, rows: picked, from: from };
    };

    // The lines the chosen rows cover, per side.
    var counters = function () {
      var first = sel.rows[0], last = sel.rows[sel.rows.length - 1];
      var span = function (side) {
        var start = num(first, side + '-next');
        var after = num(last, side + '-next') + (has(last, side) ? 1 : 0);
        return { start: start, len: after - start };
      };
      return { base: span('old'), head: span('new') };
    };
    var whereText = function () {
      var c = counters();
      var part = c.head.len > 0 ? c.head : c.base;
      var end = part.start + part.len - 1;
      return sel.table.getAttribute('data-diffnote-file') + ':' + (end > part.start ? part.start + '-' + end : part.start);
    };

    var closeComposer = function () {
      if (composer) composer.remove();
      composer = null;
      unchoose();
      sel = null;
      slice.call(document.querySelectorAll('.diffnote-compose-wrap')).forEach(function (w) { w.remove(); });
    };

    var openComposer = function () {
      var draft = composer ? composer.querySelector('textarea').value : '';
      if (composer) composer.remove();
      slice.call(document.querySelectorAll('.diffnote-compose-wrap')).forEach(function (w) { w.remove(); });
      var last = sel.rows[sel.rows.length - 1];
      var tr = document.createElement('tr');
      tr.className = 'diffnote-composer-row';
      var td = document.createElement('td');
      td.colSpan = 3;
      td.innerHTML = '<form class="diffnote-compose"><div class="diffnote-compose__where"></div>' +
        '<textarea rows="3" placeholder="コメントを書く(Ctrl+Enter で送信)"></textarea>' +
        '<div class="diffnote-reply__buttons"><button type="submit" class="diffnote-button diffnote-button--primary">コメントする</button>' +
        '<button type="button" class="diffnote-button" data-diffnote-cancel>キャンセル</button></div></form>';
      td.querySelector('.diffnote-compose__where').textContent = whereText();
      tr.appendChild(td);
      last.after(tr);
      composer = tr;
      var box = tr.querySelector('textarea');
      box.value = draft;
      box.focus();
    };

    document.addEventListener('mousedown', function (e) {
      if (e.button !== 0) return;
      var row = rowOf(e.target);
      if (!row) return;
      var table = row.closest('table');
      e.preventDefault();
      // Whatever comment's range was shown gives way to the choice.
      pinned = null;
      clear();
      if (e.shiftKey && sel && sel.table === table) {
        choose(table, sel.from, row);
      } else {
        choose(table, row, row);
      }
      dragging = true;
      document.body.classList.add('is-selecting');
    });
    document.addEventListener('mouseover', function (e) {
      if (!dragging) return;
      var row = rowOf(e.target);
      if (row && row.closest('table') === sel.table) choose(sel.table, sel.from, row);
    });
    document.addEventListener('mouseup', function () {
      if (!dragging) return;
      dragging = false;
      document.body.classList.remove('is-selecting');
      if (sel) openComposer();
    });

    var sendThread = function (form) {
      var box = form.querySelector('textarea');
      var text = box.value.trim();
      // A box for a file or the whole review says so; the others are for lines.
      var scope = form.getAttribute('data-diffnote-scope');
      if (!text || form.classList.contains('is-sending') || (!scope && !sel)) return;
      var request;
      if (scope) {
        request = {
          scope: scope, body: text,
          revision: +form.getAttribute('data-diffnote-revision'),
          file: form.getAttribute('data-diffnote-target') || undefined
        };
      } else {
        var c = counters();
        var table = sel.table;
        request = {
          revision: +table.closest('.diffnote-revision').getAttribute('data-diffnote-revision'),
          file: table.getAttribute('data-diffnote-file'),
          base: c.base, head: c.head, body: text
        };
      }
      form.classList.add('is-sending');
      box.disabled = true;
      var cell = form.parentNode;
      form.style.display = 'none';
      var pending = document.createElement('article');
      pending.className = 'diffnote-comment is-pending';
      pending.innerHTML = '<p class="diffnote-comment__author">保存中…</p><div class="diffnote-comment__body"></div>';
      pending.querySelector('.diffnote-comment__body').textContent = text;
      cell.appendChild(pending);
      post('/api/threads', request).then(function (res) {
        pending.remove();
        form.style.display = '';
        form.classList.remove('is-sending');
        box.disabled = false;
        if (!res.ok) {
          showError(form, res.error || '保存できませんでした');
          box.focus();
          return;
        }
        closeComposer();
        applyNewThread(res);
      });
    };

    var rowAt = function (table, ref) {
      if (ref.new !== null && ref.new !== undefined) return table.querySelector('tr[data-diffnote-new="' + ref.new + '"]');
      return table.querySelector('tr[data-diffnote-old="' + ref.old + '"]');
    };
    var tableOf = function (section, file) {
      return slice.call(section.querySelectorAll('table[data-diffnote-file]')).filter(function (t) {
        return t.getAttribute('data-diffnote-file') === file;
      })[0];
    };
    var parseRow = function (html) {
      var tpl = document.createElement('template');
      tpl.innerHTML = '<table><tbody>' + html + '</tbody></table>';
      return tpl.content.querySelector('tr');
    };
    var setCounts = function (open, all) {
      slice.call(document.querySelectorAll('.diffnote-side .diffnote-badge[title]')).forEach(function (b) {
        b.textContent = open + ' / ' + all;
      });
      var summary = document.querySelector('.diffnote-summary p');
      if (summary) summary.textContent = 'スレッド ' + all + ' 件(解決済み ' + (all - open) + ' 件)';
    };

    // The new thread is put into the view it was written in; the other views
    // are marked stale and drawn again when they are opened.
    var insertListItem = function (section, p) {
      var ol = section.querySelector('.diffnote-threadlist ol');
      if (!ol) return;
      var tpl = document.createElement('template');
      tpl.innerHTML = p.list_item.trim();
      var item = tpl.content.firstElementChild;
      var before = p.list_before && ol.querySelector('a[data-diffnote-jump=' + JSON.stringify(p.list_before) + ']');
      ol.insertBefore(item, before ? before.parentElement : null);
    };

    var applyNewThread = function (res) {
      var section = document.getElementById('rev-' + res.revision);
      views.forEach(function (v, k) { if (k !== res.revision) v.setAttribute('data-stale', ''); });
      setCounts(res.open, res.all);
      if (res.reload || !res.patch) { refreshView(res.revision); return; }
      if (active) clear();
      var p = res.patch;
      if (p.kind === 'card') {
        // A review-wide or a file's thread: a card at the end of its place.
        var owner = p.file === null || p.file === undefined
          ? section.querySelector('[data-diffnote-global]')
          : slice.call(section.querySelectorAll('section.diffnote-file')).filter(function (s) {
              return s.getAttribute('data-diffnote-file') === p.file;
            })[0];
        var cards = owner && owner.querySelector('[data-diffnote-cards]');
        if (!cards) { refreshView(res.revision); return; }
        var holder = document.createElement('template');
        holder.innerHTML = p.card.trim();
        var fresh = holder.content.firstElementChild;
        cards.appendChild(fresh);
        var details = fresh.closest('details.diffnote-file, .diffnote-file > details');
        if (details) details.open = true;
        insertListItem(section, p);
        syncResolved();
        fresh.scrollIntoView({ block: 'nearest' });
        return;
      }
      var table = tableOf(section, p.after.file);
      var after = table && rowAt(table, p.after);
      if (!after) { refreshView(res.revision); return; }
      p.rows.forEach(function (m) {
        var t = tableOf(section, m.row.file);
        var tr = t && rowAt(t, m.row);
        if (!tr) return;
        tr.classList.add('diffnote-line--commented');
        tr.setAttribute('data-diffnote-threads', m.threads);
        tr.style.setProperty('--diffnote-bars', m.bars);
      });
      var at = after.nextElementSibling;
      while (at && at.classList.contains('diffnote-thread-row')) { at = at.nextElementSibling; }
      var row = parseRow(p.card_row);
      after.parentNode.insertBefore(row, at);
      insertListItem(section, p);
      syncResolved();
      var card = row.querySelector('.diffnote-thread');
      if (card) {
        pinned = card.getAttribute('data-diffnote-thread-id');
        activate(section, pinned);
        card.scrollIntoView({ block: 'nearest' });
      }
    };

    document.addEventListener('submit', function (e) {
      var form = e.target.closest ? e.target.closest('.diffnote-compose') : null;
      if (!form) return;
      e.preventDefault();
      sendThread(form);
    });
    document.addEventListener('keydown', function (e) {
      var form = e.target.closest ? e.target.closest('.diffnote-compose') : null;
      if (form && e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); sendThread(form); }
      if (e.key === 'Escape' && (composer || document.querySelector('.diffnote-compose-wrap'))) closeComposer();
    });
    document.addEventListener('click', function (e) {
      if (e.target.closest && e.target.closest('[data-diffnote-cancel]')) closeComposer();
    });

    // "Comment on this file" / "on the whole review": a box at the top of the
    // file (opened if it was folded) or of the page.
    var openScopeComposer = function (button) {
      var scope = button.getAttribute('data-diffnote-add');
      var section = button.closest('.diffnote-revision');
      var owner = scope === 'global' ? button.closest('[data-diffnote-global]') : button.closest('section.diffnote-file');
      var file = scope === 'file' ? owner.getAttribute('data-diffnote-file') : '';
      closeComposer();
      var wrap = document.createElement('div');
      wrap.className = 'diffnote-compose-wrap';
      wrap.innerHTML = '<form class="diffnote-compose"><div class="diffnote-compose__where"></div>' +
        '<textarea rows="3" placeholder="コメントを書く(Ctrl+Enter で送信)"></textarea>' +
        '<div class="diffnote-reply__buttons"><button type="submit" class="diffnote-button diffnote-button--primary">コメントする</button>' +
        '<button type="button" class="diffnote-button" data-diffnote-cancel>キャンセル</button></div></form>';
      var form = wrap.querySelector('form');
      form.setAttribute('data-diffnote-scope', scope);
      form.setAttribute('data-diffnote-revision', section.getAttribute('data-diffnote-revision'));
      if (file) form.setAttribute('data-diffnote-target', file);
      wrap.querySelector('.diffnote-compose__where').textContent = scope === 'global' ? 'レビュー全体へのコメント' : file + ' へのコメント';
      if (scope === 'global') {
        button.parentElement.after(wrap);
      } else {
        var details = owner.querySelector('details');
        details.open = true;
        details.querySelector('summary').after(wrap);
      }
      wrap.querySelector('textarea').focus();
    };
    document.addEventListener('click', function (e) {
      var button = e.target.closest ? e.target.closest('[data-diffnote-add]') : null;
      if (!button) return;
      e.preventDefault();
      e.stopPropagation();
      openScopeComposer(button);
    }, true);

    // --- Other stored files: opened to look at, and to comment on ---------------
    // Nothing is recorded by opening one; a comment on it is what keeps it.
    var sectionOf = function (el) { return el.closest('.diffnote-revision'); };
    var revisionOf = function (el) { return sectionOf(el).getAttribute('data-diffnote-revision'); };
    var getJSON = function (url) {
      return fetch(url, { credentials: 'same-origin' }).then(function (r) { return r.json(); }, function () {
        return { ok: false, error: 'サーバーに接続できませんでした' };
      });
    };
    var loadTree = function (holder, revision, dir, query) {
      holder.textContent = '読み込み中…';
      return getJSON('/api/files/' + revision + '/tree?dir=' + encodeURIComponent(dir) + '&q=' + encodeURIComponent(query || '')).then(function (res) {
        if (!res.ok) { holder.textContent = res.error || '読み込めませんでした'; return; }
        holder.innerHTML = res.html;
      });
    };
    // Folded lists are read when first opened (a directory of thousands of
    // files costs nothing until then).
    document.addEventListener('toggle', function (e) {
      var d = e.target;
      if (!d.open || !d.matches) return;
      if (d.matches('[data-diffnote-tree]')) {
        var list = d.querySelector('[data-diffnote-tree-list]');
        if (!list.hasChildNodes()) loadTree(list, revisionOf(d), '', '');
      } else if (d.matches('[data-diffnote-dir]')) {
        var kids = d.querySelector('[data-diffnote-children]');
        if (!kids.hasChildNodes()) loadTree(kids, revisionOf(d), d.getAttribute('data-diffnote-dir'), '');
      }
    }, true);
    var searchTimer = null;
    document.addEventListener('input', function (e) {
      var box = e.target;
      if (!box.matches || !box.matches('.diffnote-tree__search')) return;
      clearTimeout(searchTimer);
      searchTimer = setTimeout(function () {
        loadTree(box.parentElement.querySelector('[data-diffnote-tree-list]'), revisionOf(box), '', box.value);
      }, 250);
    });
    var openFile = function (button) {
      var section = sectionOf(button);
      var path = button.getAttribute('data-diffnote-open');
      var existing = slice.call(section.querySelectorAll('section.diffnote-file')).filter(function (s) {
        return s.getAttribute('data-diffnote-file') === path;
      })[0];
      if (existing) { existing.querySelector('details').open = true; existing.scrollIntoView({ block: 'start' }); return; }
      getJSON('/api/files/' + revisionOf(button) + '/open?path=' + encodeURIComponent(path)).then(function (res) {
        if (!res.ok) { showError(button.closest('.diffnote-tree'), res.error || '開けませんでした'); return; }
        var tpl = document.createElement('template');
        tpl.innerHTML = res.html.trim();
        var fresh = tpl.content.firstElementChild;
        var files = section.querySelectorAll('section.diffnote-file');
        files[files.length - 1].after(fresh);
        var ul = section.querySelector('.diffnote-filelist ul');
        if (ul) {
          var li = document.createElement('template');
          li.innerHTML = res.list_item.trim();
          ul.appendChild(li.content.firstElementChild);
        }
        fresh.scrollIntoView({ block: 'start' });
      });
    };
    document.addEventListener('click', function (e) {
      var t = e.target.closest ? e.target : null;
      if (!t) return;
      var open = t.closest('[data-diffnote-open]');
      if (open) { openFile(open); return; }
      var close = t.closest('[data-diffnote-close]');
      if (close) {
        e.preventDefault();
        var sec = close.closest('section.diffnote-file');
        slice.call(sectionOf(sec).querySelectorAll('.diffnote-filelist a')).forEach(function (a) {
          if (a.getAttribute('href') === '#' + sec.id) a.parentElement.remove();
        });
        if (sec.contains(composer) || sec.querySelector('.diffnote-compose-wrap')) closeComposer();
        sec.remove();
        return;
      }
      var more = t.closest('[data-diffnote-more]');
      if (more) {
        var row = more.closest('tr');
        more.disabled = true;
        getJSON('/api/files/' + revisionOf(more) + '/more?path=' + encodeURIComponent(more.getAttribute('data-path')) + '&from=' + more.getAttribute('data-from')).then(function (res) {
          if (!res.ok) { more.disabled = false; return; }
          var tpl = document.createElement('template');
          tpl.innerHTML = '<table><tbody>' + res.html + '</tbody></table>';
          slice.call(tpl.content.querySelectorAll('tr')).forEach(function (tr) { row.parentNode.insertBefore(tr, row); });
          if (res.next) {
            more.setAttribute('data-from', res.next);
            more.textContent = '続きを表示(' + res.next + ' 行目から)';
            more.disabled = false;
          } else {
            row.remove();
          }
        });
      }
    });
  }

  // --- The file list follows what is on screen ---------------------------
  var watchers = [];
  function watchFiles(k) {
    var v = views[k];
    if (watchers[k]) watchers[k].disconnect();
    if (!('IntersectionObserver' in window)) return;
    var byId = {};
    slice.call(v.querySelectorAll('.diffnote-filelist a')).forEach(function (a) {
      byId[(a.getAttribute('href') || '').slice(1)] = a;
    });
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (en) {
        var a = byId[en.target.id];
        if (a) a.classList.toggle('is-visible', en.isIntersecting);
      });
    }, { rootMargin: '-48px 0px -55% 0px' });
    slice.call(v.querySelectorAll('.diffnote-file')).forEach(function (f) { io.observe(f); });
    watchers[k] = io;
  }
  views.forEach(function (v, k) { watchFiles(k); });
  if (document.body.hasAttribute('data-diffnote-api')) {
    // A view the page can't patch (or that another change made stale) is
    // fetched again and put in place of the old one.
    refreshView = function (k) {
      return fetch('/api/views/' + k, { credentials: 'same-origin' }).then(function (r) { return r.json(); }).then(function (res) {
        if (!res.ok) return;
        if (active) clear();
        views[k].innerHTML = res.html;
        views[k].removeAttribute('data-stale');
        watchFiles(k);
        syncResolved();
      });
    };
  }
  syncResolved();
})();
