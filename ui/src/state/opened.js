// Files opened to look at, which are not in the model.
import { useMemo, useRef, useState } from 'preact/hooks';
import { transport } from '../transport.ts';
import { htmlId } from '../dom.ts';

// The files opened to look at, per revision, and the lines of each read so
// far. Kept here, not in the model: nothing is recorded by opening one.
export function useOpened(interactive) {
  var _ = useState({});
  var byRev = _[0];
  var setByRev = _[1];
  var ref = useRef(byRev);
  ref.current = byRev;
  return useMemo(function () {
    if (!interactive) return null;
    var update = function (rev, fn) {
      setByRev(function (cur) {
        var next = Object.assign({}, cur);
        next[rev] = fn(cur[rev] || []);
        return next;
      });
    };
    var show = function (rev, path) {
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
        return transport.get('/api/files/' + rev + '/open?json=1&path=' + encodeURIComponent(path)).then(function (res) {
          if (!res.ok) return res;
          update(rev, function (list) { return list.concat([res.file]); });
          setTimeout(function () { show(rev, path); }, 0);
          return res;
        });
      },
      close: function (rev, path) {
        update(rev, function (list) { return list.filter(function (f) { return f.path !== path; }); });
      },
      more: function (rev, path) {
        var file = (ref.current[rev] || []).filter(function (f) { return f.path === path; })[0];
        if (!file || !file.next) return Promise.resolve({ ok: true });
        return transport.get('/api/files/' + rev + '/more?json=1&path=' + encodeURIComponent(path) + '&from=' + file.next).then(function (res) {
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
