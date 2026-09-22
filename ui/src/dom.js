// Small things about the document itself.
import { useEffect, useLayoutEffect, useState } from 'preact/hooks';

// An id for a path (as the older page made it): letters and digits, else `-`.
export function htmlId(s) {
  return s.replace(/[^A-Za-z0-9]/g, '-');
}

// A value that can be changed on the spot: its display (the children) and a
// button to edit it, which turns into a box to write the new value in.
// A box that grows and shrinks with what is written in it (wrapped lines
// too), up to what the style allows; below what `rows` gives it, it doesn't go.
function growTo(el) {
  if (!el || !el.offsetParent) return;
  el.style.height = 'auto';
  var css = window.getComputedStyle(el);
  var px = function (v) { return parseFloat(v) || 0; };
  var height = css.boxSizing === 'border-box'
    ? el.scrollHeight + px(css.borderTopWidth) + px(css.borderBottomWidth)
    : el.scrollHeight - px(css.paddingTop) - px(css.paddingBottom);
  el.style.height = height + 'px';
}

export function useAutoGrow(ref, value, live) {
  useLayoutEffect(function () { growTo(ref.current); }, [value, live]);
  useEffect(function () {
    var on = function () { growTo(ref.current); };
    window.addEventListener('resize', on);
    return function () { window.removeEventListener('resize', on); };
  }, []);
}

// Whether the window is wide enough for two columns of code.
export function useWide() {
  var query = '(min-width: 900px)';
  var _ = useState(window.matchMedia(query).matches);
  var wide = _[0];
  var setWide = _[1];
  useEffect(function () {
    var mq = window.matchMedia(query);
    var on = function () { setWide(mq.matches); };
    if (mq.addEventListener) mq.addEventListener('change', on);
    else mq.addListener(on);
    return function () {
      if (mq.removeEventListener) mq.removeEventListener('change', on);
      else mq.removeListener(on);
    };
  }, []);
  return wide;
}
