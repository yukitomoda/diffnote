// The review the page shows, and what can be done to it.
import { useCallback, useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { server } from '../transport.ts';
import type { ThreadData, ViewModel } from '../model.ts';
import type { Answer } from '../model.ts';
import type { Actions, ChangeAnswer } from './contexts.ts';

/** What can be done, before the page adds which comments it may change. */
type Changes = Omit<Actions, 'editable' | 'changed' | 'author'>;

/** A change the server answers with the whole model (it may take a thread away). */
type WholeModel = Answer<{ model: ViewModel }>;

/**
 * A change to one thread: the answer says what stamp the review had before it
 * (so the page can tell whether it has missed anything), and the thread as it
 * now is.
 */
type ThreadChange = Answer<{
  before: string;
  thread_data: ThreadData;
  stamp: string;
  editable: string[];
  changed: string[];
}>;

/** The review as the page holds it, and what may be done to it. */
export interface Review {
  model: ViewModel;
  /** `null` on an exported page: it can only be read. */
  actions: Actions | null;
  /** Whether the target has something new to take in. */
  pending: boolean;
  reload(): Promise<ChangeAnswer>;
}

// The model, and (on the served page) the changes that can be made to it.
// A change is shown at once and put right by the server's answer. The answer
// says what stamp the review had before the change: if that isn't the page's
// (the review had changed under it: another `diffnote serve`, another tab), the whole
// model is fetched again -- as it is whenever the window is looked at again
// and the review's stamp is not the page's.
export function useReview(initial: ViewModel): Review {
  var _m = useState(initial);
  var model = _m[0];
  var setModel = _m[1];
  var ref = useRef(model);
  ref.current = model;
  // Whether the target has something new to take in (asked when the page opens
  // and whenever the window is looked at again).
  var _p = useState(false);
  var pending = _p[0];
  var setPending = _p[1];

  var reloadModel = useCallback(function () {
    return server().get<{ model: ViewModel }>('/api/model').then(function (res) {
      if (res.ok) setModel(res.model);
      return res;
    });
  }, []);

  var actions: Changes | null = useMemo(function () {
    if (!initial.interactive) return null;
    var replace = function (thread: ThreadData) {
      return function (cur: ViewModel): ViewModel {
        return Object.assign({}, cur, {
          threads: cur.threads.map(function (t) { return t.id === thread.id ? thread : t; }),
        });
      };
    };
    // What a change's answer does to the page.
    var settle = function (res: ThreadChange & { ok: true }) {
      if (res.before !== ref.current.stamp) {
        reloadModel();
        return;
      }
      setModel(function (cur) {
        return Object.assign({}, replace(res.thread_data)(cur), { stamp: res.stamp, editable: res.editable, changed: res.changed });
      });
    };
    var latest: Record<string, number> = {};
    // A change whose answer is the whole model (it may take a thread away).
    var whole = function (res: WholeModel) {
      if (res.ok) setModel(res.model);
      return res;
    };
    return {
      reply: function (id, text) {
        return server().post<ThreadChange>('/api/threads/' + id + '/replies', { body: text }).then(function (res) {
          if (res.ok) settle(res);
          return res;
        });
      },
      // A new thread: the answer has the whole model, with it placed.
      create: function (request) {
        return server().post<{ model: ViewModel }>('/api/threads', request).then(whole);
      },
      // Comments added since the server started can be rewritten or taken out.
      edit: function (id, text) {
        return server().post<{ model: ViewModel }>('/api/comments/' + id + '/edit', { body: text }).then(whole);
      },
      remove: function (id) {
        return server().post<{ model: ViewModel }>('/api/comments/' + id + '/delete').then(whole);
      },
      // The signed-in name reacting to a comment with an emoji (or taking it back).
      react: function (id, emoji) {
        return server().post<{ model: ViewModel }>('/api/comments/' + id + '/react', { emoji: emoji }).then(whole);
      },
      // The review's settings (the ones given; the answer is the whole model).
      saveSettings: function (settings) {
        return server().post<{ model: ViewModel }>('/api/settings', settings).then(whole);
      },
      // Takes an image or another attached file out of the bundle. What the
      // comments say is left as it was, so a link to it simply goes nowhere.
      removeAttached: function (attached) {
        var where = attached.kind === 'image' ? '/api/images/' : '/api/attachments/';
        return server().post<{ model: ViewModel }>(where + attached.id + '/delete').then(whole);
      },
      // This machine's user settings (author name; kept for every review, not
      // only this one): the answer is the whole model, with the name applied
      // for the rest of this session too.
      saveUserSettings: function (author) {
        return server().post<{ model: ViewModel }>('/api/user-settings', { author: author }).then(whole);
      },
      // What was added to the target since the server started becomes a new
      // revision (the answer says what was done; the page keeps its place).
      refresh: function () {
        return server().post<{ model: ViewModel }>('/api/refresh').then(function (res) {
          if (res.ok) {
            setModel(res.model);
            setPending(false);
          }
          return res;
        });
      },
      setResolved: function (id, resolved) {
        var before = ref.current.threads.filter(function (t) { return t.id === id; })[0];
        setModel(replace(Object.assign({}, before, { resolved: resolved })));
        // Pressed again before the answer came: only the last answer says how
        // the thread is (an earlier one would flip it back for a moment).
        var n = (latest[id] = (latest[id] || 0) + 1);
        return server().post<ThreadChange>('/api/threads/' + id + '/' + (resolved ? 'resolve' : 'reopen')).then(function (res) {
          if (latest[id] !== n) {
            if (res.ok) setModel(function (cur) { return Object.assign({}, cur, { stamp: res.stamp, editable: res.editable, changed: res.changed }); });
          } else if (res.ok) settle(res);
          else setModel(replace(before));
          return res;
        });
      },
    };
  }, [initial.interactive]);

  useEffect(function () {
    if (!initial.interactive) return undefined;
    var check = function () {
      if (document.hidden) return;
      server().get<{ stamp: string; pending: boolean }>('/api/version').then(function (res) {
        if (!res.ok) return;
        setPending(!!res.pending);
        if (res.stamp !== ref.current.stamp) reloadModel();
      });
    };
    check();
    window.addEventListener('focus', check);
    document.addEventListener('visibilitychange', check);
    return function () {
      window.removeEventListener('focus', check);
      document.removeEventListener('visibilitychange', check);
    };
  }, [initial.interactive]);

  // What the page may change, and which comments those are.
  var full: Actions | null = useMemo(function () {
    if (!actions) return null;
    return Object.assign({}, actions, { editable: new Set(model.editable || []), changed: new Set(model.changed || []), author: model.author });
  }, [actions, model.editable, model.changed, model.author]);
  return { model: model, actions: full, pending: pending, reload: reloadModel };
}
