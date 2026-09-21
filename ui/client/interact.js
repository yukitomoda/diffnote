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
  function install() {
    if (installed) return;
    installed = true;
    document.addEventListener('mouseover', function (e) {
      if (pinned) return;
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
    document.addEventListener('keydown', function (e) {
      if (e.key === 'Escape') {
        pinned = null;
        clear();
      }
    });
  }

  D.interact = {
    install: install,
    // Let go of whatever is shown or pinned (the page is about to change).
    reset: function () {
      pinned = null;
      clear();
    },
  };
})(window.Diffnote);
