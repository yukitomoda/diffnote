// Files opened to look at, which are not in the model.
import { useMemo, useRef, useState } from 'preact/hooks';
import { server } from '../transport.ts';
import type { Hunk, OpenedFile } from '../model.ts';
import type { Opened } from './contexts.ts';
import { htmlId } from '../dom.ts';

// The files opened to look at, per revision, and the lines of each read so
// far. Kept here, not in the model: nothing is recorded by opening one.
export function useOpened(interactive: boolean): Opened | null {
  var _ = useState<Record<number, OpenedFile[]>>({});
  var byRev = _[0];
  var setByRev = _[1];
  var ref = useRef(byRev);
  ref.current = byRev;
  return useMemo(function (): Opened | null {
    if (!interactive) return null;
    var update = function (rev: number, fn: (list: OpenedFile[]) => OpenedFile[]) {
      setByRev(function (cur) {
        var next = Object.assign({}, cur);
        next[rev] = fn(cur[rev] || []);
        return next;
      });
    };
    var show = function (rev: number, path: string) {
      var el = document.getElementById('r' + rev + '-file-' + htmlId(path));
      if (!el) return;
      var d = el.querySelector('details');
      if (d) d.open = true;
      el.scrollIntoView({ block: 'start' });
    };
    return {
      byRev: byRev,
      open: function (rev, path) {
        var here = (ref.current[rev] || []).some(function (f) { return f.path === path; });
        if (here || document.getElementById('r' + rev + '-file-' + htmlId(path))) {
          show(rev, path);
          return Promise.resolve({ ok: true });
        }
        return server().get<{ file: OpenedFile }>('api/files/' + rev + '/open?json=1&path=' + encodeURIComponent(path)).then(function (res) {
          if (!res.ok) return res;
          update(rev, function (list) { return list.concat([res.file]); });
          setTimeout(function () { show(rev, path); }, 0);
          return res;
        });
      },
      // Opened (if it isn't yet) and read as far as line `line`, all at once.
      reach: function (rev, path, line) {
        var had = (ref.current[rev] || []).filter(function (f) { return f.path === path; })[0];
        var first: Promise<{ ok: true; file: OpenedFile } | { ok: false; error: string }> = had
          ? Promise.resolve({ ok: true as const, file: had })
          : server().get<{ file: OpenedFile }>('api/files/' + rev + '/open?json=1&path=' + encodeURIComponent(path));
        return first.then(function (res) {
          if (!res.ok) return res;
          var file = res.file;
          var hunks: Hunk[] = [];
          var next = file.next;
          var step = function (): Promise<unknown> {
            if (!next || next > line) return Promise.resolve();
            return server().get<{ hunk: Hunk; next: number | null }>('api/files/' + rev + '/more?json=1&path=' + encodeURIComponent(path) + '&from=' + next).then(function (more) {
              if (!more.ok) { next = null; return; }
              hunks.push(more.hunk);
              next = more.next;
              return step();
            });
          };
          return step().then(function () {
            update(rev, function (list) {
              var whole = Object.assign({}, file, { hunks: file.hunks.concat(hunks), next: next });
              return list.some(function (f) { return f.path === path; })
                ? list.map(function (f) { return f.path === path ? Object.assign({}, f, { hunks: f.hunks.concat(hunks), next: next }) : f; })
                : list.concat([whole]);
            });
            return { ok: true as const };
          });
        });
      },
      close: function (rev, path) {
        update(rev, function (list) { return list.filter(function (f) { return f.path !== path; }); });
      },
      more: function (rev, path) {
        var file = (ref.current[rev] || []).filter(function (f) { return f.path === path; })[0];
        if (!file || !file.next) return Promise.resolve({ ok: true });
        return server().get<{ hunk: Hunk; next: number | null }>('api/files/' + rev + '/more?json=1&path=' + encodeURIComponent(path) + '&from=' + file.next).then(function (res) {
          if (res.ok) {
            update(rev, function (list) {
              return list.map(function (f) {
                return f.path === path ? Object.assign({}, f, { hunks: f.hunks.concat([res.hunk]), next: res.next }) : f;
              });
            });
          }
          return res;
        });
      },
    };
  }, [interactive, byRev]);
}
