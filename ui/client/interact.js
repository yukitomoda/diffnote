// Direct handling of the page's mouse and keyboard, done on the document
// rather than through the components: showing a thread's range while it is
// under the mouse, pinning it with a click, jumping from the thread list, and
// the copy buttons. (Marking the lines of a range touches only those lines, so
// a hover doesn't make the page draw itself again.) It reads what the
// components write: `data-diffnote-thread-id` and `data-diffnote-color` on a
// thread's card, and `data-diffnote-threads` on the lines a thread covers.
(function (D) {
  'use strict';
  var slice = Array.prototype.slice;
  var THREAD = '[data-diffnote-thread-id]';
  var LINE = 'tr[data-diffnote-threads]';
  var active = null;
  var pinned = null;

  function scopeOf(el) {
    return el.closest('.diffnote-revision') || document;
  }
  function cardOf(scope, id) {
    return scope.querySelector('[data-diffnote-thread-id="' + id + '"]');
  }
  function rowsOf(scope, id) {
    return slice.call(scope.querySelectorAll('tr[data-diffnote-threads~="' + id + '"]'));
  }

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
    // A thread that is hidden as resolved shows no range either: the person
    // couldn't tell what the highlight was for.
    if (document.body.classList.contains('diffnote-hide-resolved')) {
      ids = ids.filter(function (id) {
        return !document.querySelector('.diffnote-thread--resolved[data-diffnote-thread-id="' + id + '"]');
      });
    }
    var best = null;
    var size = Infinity;
    ids.forEach(function (id) {
      var n = rowsOf(scope, id).length;
      if (n < size) {
        best = id;
        size = n;
      }
    });
    return best;
  }

  function target(node) {
    return node && node.closest ? node.closest(THREAD + ', ' + LINE) : null;
  }

  // A thread in the list: open its card, bring it to the middle of the screen
  // and keep its range shown.
  function jump(link) {
    jumpTo(link.getAttribute('href').slice(1));
  }

  function jumpTo(id) {
    var card = document.getElementById(id);
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
      button.textContent = D.lib.m('ui.copied');
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
      try {
        document.execCommand('copy');
        done();
      } catch (err) {
        /* nothing to do */
      }
      document.body.removeChild(ta);
    }
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(done, fallback);
    } else {
      fallback();
    }
  }

  var installed = false;
  // An image of a comment, shown by itself over the page at its own size.
  // Built here rather than as a component, so it works the same in an exported
  // page (where the image is a `data:` address) as in a served one.
  var zoomed = null;
  function closeZoom() {
    if (!zoomed) return;
    var back = zoomed.back;
    zoomed.box.remove();
    document.body.style.overflow = zoomed.overflow;
    zoomed = null;
    if (back && back.isConnected) back.focus();
  }
  function openZoom(image) {
    closeZoom();
    var box = document.createElement('div');
    box.className = 'diffnote-zoom';
    box.setAttribute('data-diffnote-zoom', '');
    box.setAttribute('role', 'dialog');
    box.setAttribute('aria-modal', 'true');
    if (image.alt) box.setAttribute('aria-label', image.alt);
    var full = document.createElement('img');
    full.src = image.currentSrc || image.src;
    full.alt = image.alt || '';
    full.setAttribute('data-diffnote-zoom-image', '');
    // Its own size; too big for the window, the box scrolls. Pressing it fits
    // it to the window instead, and again brings it back (the cursor says so).
    var fitted = false;
    full.addEventListener('click', function (e) {
      e.stopPropagation();
      // One the window can hold has nothing to shrink to: pressing it (the
      // cursor says zoom-out) is being done with it.
      var overflows = box.scrollWidth > box.clientWidth || box.scrollHeight > box.clientHeight;
      if (!fitted && !overflows) {
        closeZoom();
        return;
      }
      fitted = !fitted;
      box.classList.toggle('is-fitted', fitted);
    });
    var close = document.createElement('button');
    close.type = 'button';
    close.className = 'diffnote-zoom__close';
    close.setAttribute('data-diffnote-zoom-close', '');
    close.setAttribute('aria-label', D.lib.m('ui.image.close'));
    close.textContent = '×';
    close.addEventListener('click', closeZoom);
    box.addEventListener('click', closeZoom);
    box.appendChild(full);
    box.appendChild(close);
    zoomed = { box: box, back: document.activeElement, overflow: document.body.style.overflow };
    document.body.style.overflow = 'hidden';
    document.body.appendChild(box);
    close.focus();
  }

  function install() {
    if (installed) return;
    installed = true;
    document.addEventListener('click', function (e) {
      // Not one that is itself a link: that click belongs to the link.
      var image = e.target.closest ? e.target.closest('img.diffnote-image') : null;
      if (!image || image.closest('a')) return;
      e.preventDefault();
      openZoom(image);
    });
    document.addEventListener('mouseover', function (e) {
      // While lines are being chosen, no comment's range is shown.
      if (pinned || document.body.classList.contains('is-selecting')) return;
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
    document.addEventListener(
      'click',
      function (e) {
        var button = e.target.closest ? e.target.closest('[data-diffnote-copy]') : null;
        if (!button) return;
        e.preventDefault();
        e.stopPropagation();
        copyText(button.getAttribute('data-diffnote-copy'), button);
      },
      true
    );
    document.addEventListener('click', function (e) {
      var link = e.target.closest ? e.target.closest('a[data-diffnote-jump]') : null;
      if (link) {
        e.preventDefault();
        jump(link);
        return;
      }
      // A line number on the served page starts a choice of lines, not a pin.
      if (
        document.body.hasAttribute('data-diffnote-api') &&
        e.target.closest &&
        e.target.closest('.diffnote-line__gutter-old, .diffnote-line__gutter-new')
      )
        return;
      var el = target(e.target);
      if (!el) {
        pinned = null;
        clear();
        return;
      }
      var scope = scopeOf(el);
      var id = pick(scope, el);
      if (!id) return;
      if (pinned === id) {
        pinned = null;
        return;
      }
      pinned = id;
      activate(scope, id);
    });
    // Side by side, text is selected (to copy) on the side the press was on: the
    // other side's cells (and the line numbers, which style.css never lets be
    // selected) are left out. Kept until the next press, since the copy comes
    // after the drag.
    document.addEventListener('mousedown', function (e) {
      var tables = document.querySelectorAll('[data-diffnote-copy-side]');
      for (var i = 0; i < tables.length; i++) tables[i].removeAttribute('data-diffnote-copy-side');
      var cell = e.target.closest ? e.target.closest('.diffnote-diff--split .diffnote-split-row > td.diffnote-line__content') : null;
      if (!cell) return;
      var index = Array.prototype.indexOf.call(cell.parentNode.children, cell);
      cell.closest('table').setAttribute('data-diffnote-copy-side', index === 1 ? 'old' : 'new');
    });
    // The text copied is of that side only, worked out here rather than left to
    // the browser (which is not consistent about text that can't be selected):
    // the chosen part of the side's cells, a line each.
    document.addEventListener('copy', function (e) {
      var sel = window.getSelection();
      if (!sel || sel.rangeCount === 0 || sel.isCollapsed || !e.clipboardData) return;
      var table = document.querySelector('.diffnote-diff--split[data-diffnote-copy-side]');
      if (!table) return;
      var column = table.getAttribute('data-diffnote-copy-side') === 'old' ? 1 : 3;
      var range = sel.getRangeAt(0);
      if (!range.intersectsNode(table)) return;
      var lines = [];
      var rows = table.querySelectorAll('.diffnote-split-row');
      for (var i = 0; i < rows.length; i++) {
        var cell = rows[i].children[column];
        if (!cell || !range.intersectsNode(cell)) continue;
        var part = document.createRange();
        part.selectNodeContents(cell);
        if (cell.contains(range.startContainer)) part.setStart(range.startContainer, range.startOffset);
        if (cell.contains(range.endContainer)) part.setEnd(range.endContainer, range.endOffset);
        // A row the choice only touches at its edge has nothing of this side.
        if (cell.textContent === '' && !cell.querySelector('code')) continue;
        lines.push(part.toString().replace(/\n$/, ''));
      }
      e.clipboardData.setData('text/plain', lines.join('\n'));
      e.preventDefault();
    });
    document.addEventListener('keydown', function (e) {
      if (e.key !== 'Escape') return;
      // An image shown by itself takes the key: closing it is what Escape
      // means while it is up.
      if (zoomed) {
        e.stopPropagation();
        closeZoom();
        return;
      }
      pinned = null;
      clear();
    });
  }

  D.interact = {
    install: install,
    // Let go of whatever is shown or pinned (the page is about to change).
    reset: function () {
      pinned = null;
      clear();
    },
    // Show a file: open it, scroll to it and mark it for a moment (once the page
    // has drawn it, if it is being brought back).
    showFile: function (rev, path) {
      var tries = 0;
      var look = function () {
        var section = Array.prototype.filter.call(
          document.querySelectorAll('#rev-' + rev + ' section.diffnote-file'),
          function (e) { return e.getAttribute('data-diffnote-file') === path; }
        )[0];
        if (!section) {
          if (++tries < 40) requestAnimationFrame(look);
          return;
        }
        var details = section.querySelector('details');
        if (details && !details.open) {
          details.open = true;
          details.dispatchEvent(new Event('toggle'));
        }
        section.scrollIntoView({ block: 'start' });
        section.classList.add('diffnote-flash');
        setTimeout(function () { section.classList.remove('diffnote-flash'); }, 1800);
      };
      requestAnimationFrame(look);
    },
    // Show lines of a file (as the side, `old` or `new`, numbers them): open the file, scroll
    // to them and mark them for a moment. Lines the diff leaves out are not on
    // the page: the nearest that are shown stand for them.
    showLines: function (rev, path, side, start, end) {
      var attr = side === 'old' ? 'data-diffnote-old' : 'data-diffnote-new';
      var tries = 0;
      var look = function () {
        var section = Array.prototype.filter.call(
          document.querySelectorAll('#rev-' + rev + ' section.diffnote-file'),
          function (e) { return e.getAttribute('data-diffnote-file') === path; }
        )[0];
        var cells = section ? section.querySelectorAll('[' + attr + ']') : [];
        if (section && cells.length === 0) {
          // Not drawn until opened.
          for (var n = section.querySelector('details'); n && !n.open; ) {
            n.open = true;
            n.dispatchEvent(new Event('toggle'));
            break;
          }
        }
        if (!section || cells.length === 0) {
          if (++tries < 40) requestAnimationFrame(look);
          else if (section) section.scrollIntoView({ block: 'start' });
          return;
        }
        for (var d = section.querySelector('details'); d; d = d.parentElement && d.parentElement.closest('details')) d.open = true;
        var inside = [];
        var before = null;
        var after = null;
        Array.prototype.forEach.call(cells, function (c) {
          var v = +c.getAttribute(attr);
          if (v >= start && v <= end) inside.push(c);
          else if (v < start) before = c;
          else if (!after) after = c;
        });
        var marked = inside.length ? inside : [after || before];
        var rows = marked.map(function (c) { return c.closest('tr'); });
        rows.forEach(function (r) { r.classList.add('diffnote-linked'); });
        setTimeout(function () { rows.forEach(function (r) { r.classList.remove('diffnote-linked'); }); }, 2600);
        rows[0].scrollIntoView({ block: 'center' });
      };
      requestAnimationFrame(look);
    },
    // Go to the element with this id once the page has drawn it (it is being
    // brought back), looking for it for a moment.
    jumpWhenShown: function (id) {
      var tries = 0;
      var look = function () {
        if (document.getElementById(id)) jumpTo(id);
        else if (++tries < 30) requestAnimationFrame(look);
      };
      requestAnimationFrame(look);
    },
  };
})(window.Diffnote);
