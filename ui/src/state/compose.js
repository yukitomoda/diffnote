// Choosing lines, and the box for a new thread.
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { interact } from '../interact.js';
import { lib } from '../lib.ts';

// Lines being chosen (pressing a line number, dragging, Shift+click), or a
// box open for a file or the review, and what is written in it. `null` when
// the page can't change the review.
export function useCompose(actions, current) {
  var _s = useState(null);
  var sel = _s[0];
  var setSel = _s[1];
  var _g = useState(false);
  var selecting = _g[0];
  var setSelecting = _g[1];
  var _o = useState(null);
  var scope = _o[0];
  var setScope = _o[1];
  var _d = useState('');
  var draft = _d[0];
  var setDraft = _d[1];
  var _p = useState(false);
  var pending = _p[0];
  var setPending = _p[1];
  var _e = useState('');
  var error = _e[0];
  var setError = _e[1];
  var dragging = useRef(false);

  var close = function () {
    setSel(null);
    setScope(null);
    setDraft('');
    setError('');
  };
  useEffect(function () {
    var up = function () {
      if (!dragging.current) return;
      dragging.current = false;
      document.body.classList.remove('is-selecting');
      setSelecting(false);
    };
    var key = function (e) {
      if (e.key === 'Escape') close();
    };
    document.addEventListener('mouseup', up);
    document.addEventListener('keydown', key);
    return function () {
      document.removeEventListener('mouseup', up);
      document.removeEventListener('keydown', key);
    };
  }, []);
  // Another revision: what was open belonged to the one left.
  useEffect(close, [current]);

  return useMemo(function () {
    if (!actions) return null;
    return {
      sel: sel, selecting: selecting, scope: scope, draft: draft, pending: pending, error: error,
      setDraft: setDraft, close: close,
      // `side` is 'old' or 'new' where lines are chosen on one side of a side
      // by side view (else none: both sides, as in the unified view).
      begin: function (rev, path, idx, shift, side) {
        // Whatever comment's range was shown gives way to the choice.
        interact.reset();
        setScope(null);
        setError('');
        setSel(function (cur) {
          return shift && cur && cur.rev === rev && cur.path === path && cur.side === side
            ? { rev: rev, path: path, side: side, anchor: cur.anchor, to: idx }
            : { rev: rev, path: path, side: side, anchor: idx, to: idx };
        });
        dragging.current = true;
        document.body.classList.add('is-selecting');
        setSelecting(true);
      },
      // `at` is the row's index, or a function of the side that gives the
      // index of the row that side has there (or nothing).
      extend: function (at) {
        if (!dragging.current) return;
        setSel(function (cur) {
          if (!cur) return cur;
          var idx = typeof at === 'function' ? at(cur.side) : at;
          return idx == null || cur.to === idx ? cur : Object.assign({}, cur, { to: idx });
        });
      },
      openScope: function (kind, rev, path) {
        setSel(null);
        setError('');
        setScope({ kind: kind, rev: rev, path: path });
      },
      send: function (request) {
        var text = draft.trim();
        if (!text || pending) return;
        setPending(true);
        setError('');
        actions.create(Object.assign({}, request, { body: text })).then(function (res) {
          setPending(false);
          if (res.ok) close();
          else setError(res.error || lib.m('ui.save_failed'));
        });
      },
    };
  }, [actions, sel, selecting, scope, draft, pending, error]);
}
