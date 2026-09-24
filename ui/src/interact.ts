// Direct handling of the page's mouse and keyboard, done on the document
// rather than through the components: showing a thread's range while it is
// under the mouse, pinning it with a click, jumping from the thread list, and
// the copy buttons. (Marking the lines of a range touches only those lines, so
// a hover doesn't make the page draw itself again.) It reads what the
// components write: `data-diffnote-thread-id` and `data-diffnote-color` on a
// thread's card, and `data-diffnote-threads` on the lines a thread covers.
import { lib } from './lib.ts';
import { iconNode } from './icon.tsx';

var slice = Array.prototype.slice;
var THREAD = '[data-diffnote-thread-id]';
var LINE = 'tr[data-diffnote-threads]';
/** The thread whose range is marked just now, and the one pinned by a click. */
interface Marked {
  id: string;
  scope: ParentNode;
  rows: HTMLElement[];
  card: HTMLElement | null;
}
var active: Marked | null = null;
var pinned: string | null = null;

function scopeOf(el: Element): ParentNode {
  return el.closest('.diffnote-revision') || document;
}
function cardOf(scope: ParentNode, id: string): HTMLElement | null {
  return scope.querySelector('[data-diffnote-thread-id="' + id + '"]');
}
function rowsOf(scope: ParentNode, id: string): HTMLElement[] {
  return slice.call(scope.querySelectorAll('tr[data-diffnote-threads~="' + id + '"]'));
}

function clear() {
  if (!active) return;
  active.rows.forEach(function (r: HTMLElement) {
    r.classList.remove('diffnote-range', 'diffnote-range-first', 'diffnote-range-last');
    r.style.removeProperty('--rc');
  });
  if (active.card) {
    active.card.classList.remove('diffnote-hover');
    active.card.style.removeProperty('--rc');
  }
  active = null;
}

function activate(scope: ParentNode, id: string) {
  if (active && active.id === id && active.scope === scope) return;
  clear();
  var rows = rowsOf(scope, id);
  var card = cardOf(scope, id);
  var color = (card && card.getAttribute('data-diffnote-color')) || '#0969da';
  rows.forEach(function (r: HTMLElement, k: number) {
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
function pick(scope: ParentNode, el: Element) {
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

function target(node: EventTarget | null): Element | null {
  return node instanceof Element ? node.closest(THREAD + ', ' + LINE) : null;
}

// A thread in the list: open its card, bring it to the middle of the screen
// and keep its range shown.
function jump(link: Element) {
  jumpTo((link.getAttribute('href') || '').slice(1));
}

function jumpTo(id: string) {
  var card = document.getElementById(id);
  if (!card) return;
  for (var n: HTMLElement | null = card; n; n = n.parentElement) {
    if (n instanceof HTMLDetailsElement) n.open = true;
  }
  card.scrollIntoView({ block: 'center' });
  pinned = card.getAttribute('data-diffnote-thread-id');
  if (pinned) activate(scopeOf(card), pinned);
}

// Copy buttons (file paths, thread locations). Handled before anything else
// sees the click, since they sit inside <summary> elements.
function copyText(text: string, button: HTMLElement) {
  // Said by the button itself for a moment: a tick where its icon was, and
  // 「コピーしました」 as what it is called.
  function done() {
    if (button.classList.contains('is-done')) return;
    var before = Array.prototype.slice.call(button.childNodes) as Node[];
    var title = button.getAttribute('title');
    button.replaceChildren(iconNode('check'));
    button.setAttribute('title', lib.m('ui.copied'));
    button.classList.add('is-done');
    setTimeout(function () {
      button.replaceChildren.apply(button, before);
      if (title != null) button.setAttribute('title', title);
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
// page (where the image is a `data:` address) as in a served one. The 添付
// list opens it too, through `interact.zoom`.
/** The picture shown by itself, what it was opened from, and what the page's
 * own scrolling was before it. */
interface Zoomed {
  box: HTMLElement;
  image: HTMLImageElement;
  back: HTMLElement | null;
  overflow: string;
}
var zoomed: Zoomed | null = null;
function closeZoom() {
  if (!zoomed) return;
  var back = zoomed.back;
  zoomed.box.remove();
  document.body.style.overflow = zoomed.overflow;
  zoomed = null;
  if (back && back.isConnected) back.focus();
}
function openZoom(src: string, alt: string) {
  closeZoom();
  var box = document.createElement('div');
  box.className = 'diffnote-zoom';
  box.setAttribute('data-diffnote-zoom', '');
  box.setAttribute('role', 'dialog');
  box.setAttribute('aria-modal', 'true');
  if (alt) box.setAttribute('aria-label', alt);
  var full = document.createElement('img');
  full.src = src;
  full.alt = alt;
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
  close.setAttribute('aria-label', lib.m('ui.image.close'));
  close.appendChild(iconNode('close'));
  close.addEventListener('click', closeZoom);
  box.addEventListener('click', closeZoom);
  box.appendChild(full);
  box.appendChild(close);
  zoomed = { box: box, image: full, back: document.activeElement as HTMLElement | null, overflow: document.body.style.overflow };
  document.body.style.overflow = 'hidden';
  document.body.appendChild(box);
  close.focus();
}

function install() {
  if (installed) return;
  installed = true;
  document.addEventListener('click', function (e) {
    // Not one that is itself a link: that click belongs to the link.
    var image = e.target instanceof Element ? e.target.closest<HTMLImageElement>('img.diffnote-image') : null;
    if (!image || image.closest('a')) return;
    e.preventDefault();
    openZoom(image.currentSrc || image.src, image.alt || '');
  });
  // While a picture is up, Escape is the picture's: it is taken here, in the
  // capture phase, so that it never reaches whoever else listens for it (the
  // settings screen closes on Escape, and the 添付 list is one of its
  // sections).
  document.addEventListener(
    'keydown',
    function (e) {
      if (e.key !== 'Escape' || !zoomed) return;
      e.stopPropagation();
      closeZoom();
    },
    true
  );
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
      var button = e.target instanceof Element ? e.target.closest<HTMLElement>('[data-diffnote-copy]') : null;
      if (!button) return;
      e.preventDefault();
      e.stopPropagation();
      copyText(button.getAttribute('data-diffnote-copy') || '', button);
    },
    true
  );
  document.addEventListener('click', function (e) {
    var link = e.target instanceof Element ? e.target.closest('a[data-diffnote-jump]') : null;
    if (link) {
      e.preventDefault();
      jump(link);
      return;
    }
    // A line number on the served page starts a choice of lines, not a pin.
    if (
      document.body.hasAttribute('data-diffnote-api') &&
      e.target instanceof Element &&
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
    var cell = e.target instanceof Element ? e.target.closest('.diffnote-diff--split .diffnote-split-row > td.diffnote-line__content') : null;
    if (!cell) return;
    var index = Array.prototype.indexOf.call(cell.parentNode!.children, cell);
    cell.closest('table')!.setAttribute('data-diffnote-copy-side', index === 1 ? 'old' : 'new');
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
    pinned = null;
    clear();
  });
}

export const interact = {
  install: install,
  // Show a picture by itself over the page, as pressing one in a comment
  // does. For a picture that isn't in a comment to begin with (the 添付 list
  // shows thumbnails): `alt` is what a reader is told it is.
  zoom: function (src: string, alt: string) {
    openZoom(src, alt);
  },
  // Let go of whatever is shown or pinned (the page is about to change).
  reset: function () {
    pinned = null;
    clear();
  },
  // Show a file: open it, scroll to it and mark it for a moment (once the page
  // has drawn it, if it is being brought back).
  showFile: function (rev: number, path: string) {
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
  showLines: function (rev: number, path: string, side: string, start: number, end: number) {
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
      var inside: Element[] = [];
      var before: Element | null = null;
      var after: Element | null = null;
      Array.prototype.forEach.call(cells, function (c: Element) {
        var v = +(c.getAttribute(attr) || '');
        if (v >= start && v <= end) inside.push(c);
        else if (v < start) before = c;
        else if (!after) after = c;
      });
      // Nothing in the range: the line before it, or the one after.
      var nearest: Element | null = after || before;
      var marked: Element[] = inside.length ? inside : nearest ? [nearest] : [];
      var rows: HTMLElement[] = [];
      marked.forEach(function (c) {
        var row = c.closest('tr');
        if (row) rows.push(row);
      });
      if (rows.length === 0) return;
      rows.forEach(function (r) { r.classList.add('diffnote-linked'); });
      setTimeout(function () { rows.forEach(function (r) { r.classList.remove('diffnote-linked'); }); }, 2600);
      rows[0].scrollIntoView({ block: 'center' });
    };
    requestAnimationFrame(look);
  },
  // Go to the element with this id once the page has drawn it (it is being
  // brought back), looking for it for a moment.
  jumpWhenShown: function (id: string) {
    var tries = 0;
    var look = function () {
      if (document.getElementById(id)) jumpTo(id);
      else if (++tries < 30) requestAnimationFrame(look);
    };
    requestAnimationFrame(look);
  },
};
