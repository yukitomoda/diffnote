// The page: a Preact app drawn from the data in `#diffnote-data` (see
// `src/html/viewmodel.rs` for its form). It is a plain script, opened from a
// file: no modules, no requests; the libraries are in the same page.
//
// The markup (class names, data attributes) is what `ui/style.css` styles and
// what `ui/client/interact.js` and the browser tests look for.
(function (D) {
  'use strict';
  var lib = D.lib;
  var h = preact.h;
  var render = preact.render;
  var useState = preactHooks.useState;
  var useEffect = preactHooks.useEffect;
  var useLayoutEffect = preactHooks.useLayoutEffect;
  var useMemo = preactHooks.useMemo;
  var html = htm.bind(h);

  var DEFAULT_TITLE = 'diffnote レビュー';

  // What is kept between visits, where the browser lets us.
  function kept(key, fallback) {
    try {
      var v = localStorage.getItem(key);
      return v === null ? fallback : v;
    } catch (e) {
      return fallback;
    }
  }
  function keep(key, value) {
    try {
      localStorage.setItem(key, value);
    } catch (e) {
      /* not kept */
    }
  }

  // An id for a path (as the older page made it): letters and digits, else `-`.
  function htmlId(s) {
    return s.replace(/[^A-Za-z0-9]/g, '-');
  }

  function Time(props) {
    var t = new Date(props.at);
    return html`<time class="diffnote-comment__time" datetime=${props.at} title=${isNaN(t.getTime()) ? props.at : t.toLocaleString()}>${lib.formatTime(props.at)}</time>`;
  }

  var ABSENCE = {
    deleted: ' (削除された行)',
    'not-yet': ' (この版にはまだない行)',
    unknown: ' (この版にない行)',
  };

  // One thread as a card.
  function Card(props) {
    var t = props.thread;
    var p = props.placement;
    var loc = lib.location(p);
    var color = p && p.kind === 'line' ? lib.color(p.color) : null;
    var absent = p && p.kind === 'point' ? p : null;
    return html`<details
      class=${'diffnote-thread' + (t.resolved ? ' diffnote-thread--resolved' : '')}
      id=${'r' + props.rev + '-thread-' + t.id}
      data-diffnote-thread-id=${t.id}
      data-diffnote-color=${color || '#57606a'}
      open=${!t.resolved}
    >
      <summary>
        ${color && html`<span class="diffnote-thread__swatch" style=${'background:' + color}></span>`}${t.resolved ? '解決済み' : '未解決'}${loc && html` <span class="diffnote-thread__where">${loc}</span>`}${absent && ABSENCE[absent.absence]}${loc &&
        html`<button type="button" class="diffnote-copy" data-diffnote-copy=${loc} title="ファイルパスと行をコピー">コピー</button>`}
      </summary>
      ${absent && absent.was.length > 0 && html`<pre class="diffnote-deleted__snippet">${absent.was.join('\n') + '\n'}</pre>`}
      ${t.comments.map(function (c) {
        return html`<article class="diffnote-comment">
          <p class="diffnote-comment__author">${c.author}<${Time} at=${c.at} /></p>
          <div class="diffnote-comment__body" dangerouslySetInnerHTML=${{ __html: c.html }}></div>
        </article>`;
      })}
    </details>`;
  }

  // The rows of one file's diff, with the cards of the threads on them.
  function DiffTable(props) {
    var file = props.file;
    var ctx = props.ctx;
    var cover = useMemo(
      function () {
        return lib.coverage(ctx.order, ctx.placements, file.path);
      },
      [ctx.order, ctx.placements, file.path]
    );
    var after = useMemo(
      function () {
        return lib.cardsAfter(ctx.order, ctx.placements, file.path);
      },
      [ctx.order, ctx.placements, file.path]
    );
    var out = [];
    file.hunks.forEach(function (hunk, hi) {
      out.push(html`<tr class="diffnote-hunk-header" key=${'h' + hi}><td colspan="3">${hunk.header}</td></tr>`);
      hunk.rows.forEach(function (row, ri) {
        var ids = lib.covering(cover, row);
        var resolvedOnly = ctx.hideResolved && ids.length > 0 && ids.every(function (id) { return ctx.byId[id].resolved; });
        var cls =
          'diffnote-line--' + (row.k === 'c' ? 'context' : row.k === 'a' ? 'added' : 'removed') +
          (ids.length ? ' diffnote-line--commented' : '') +
          (resolvedOnly ? ' diffnote-line--resolved-only' : '');
        var colors = ids.map(function (id) { return ctx.placements[id].color; });
        out.push(html`<tr
          class=${cls}
          key=${hi + ':' + ri}
          data-diffnote-threads=${ids.length ? ids.join(' ') : undefined}
          style=${ids.length ? '--diffnote-bars: ' + lib.bars(colors) : undefined}
        >
          <td class="diffnote-line__gutter-old">${row.o != null ? row.o : ''}</td>
          <td class="diffnote-line__gutter-new">${row.n != null ? row.n : ''}</td>
          <td class="diffnote-line__content"><code dangerouslySetInnerHTML=${{ __html: row.h }}></code></td>
        </tr>`);
        lib.cardsOfRow(after, row).forEach(function (id) {
          out.push(html`<tr class="diffnote-thread-row" key=${'c' + id}><td colspan="3"><${Card} rev=${ctx.rev} thread=${ctx.byId[id]} placement=${ctx.placements[id]} /></td></tr>`);
        });
      });
    });
    return html`<div class="diffnote-diff-scroll"><table class="diffnote-diff"><tbody>${out}</tbody></table></div>`;
  }

  // The same rows side by side: what a file was on the left, what it is on the
  // right. A run of removed rows sits beside the run of added rows after it.
  // A thread's mark (its color bar) is on the gutter of the side it is on; its
  // range, when hovered, is shown on the whole row.
  function SplitTable(props) {
    var file = props.file;
    var ctx = props.ctx;
    var cover = useMemo(
      function () {
        return lib.coverage(ctx.order, ctx.placements, file.path);
      },
      [ctx.order, ctx.placements, file.path]
    );
    var after = useMemo(
      function () {
        return lib.cardsAfter(ctx.order, ctx.placements, file.path);
      },
      [ctx.order, ctx.placements, file.path]
    );
    var hidden = function (ids) {
      return ctx.hideResolved && ids.length > 0 && ids.every(function (id) { return ctx.byId[id].resolved; });
    };
    var cellKind = function (row, side) {
      if (!row) return 'empty';
      return row.k === 'c' ? 'context' : side === 'old' ? 'removed' : 'added';
    };
    var out = [];
    file.hunks.forEach(function (hunk, hi) {
      out.push(html`<tr class="diffnote-hunk-header" key=${'h' + hi}><td colspan="4">${hunk.header}</td></tr>`);
      lib.pairRows(hunk.rows).forEach(function (pair, pi) {
        var l = pair.left;
        var r = pair.right;
        var idsL = l && l.o != null ? cover.old[l.o] || [] : [];
        var idsR = r && r.n != null ? cover.new[r.n] || [] : [];
        var ids = idsL.concat(idsR.filter(function (id) { return idsL.indexOf(id) < 0; }));
        var shownL = idsL.length > 0 && !hidden(idsL);
        var shownR = idsR.length > 0 && !hidden(idsR);
        var bars = function (side) {
          return '--diffnote-bars: ' + lib.bars(side.map(function (id) { return ctx.placements[id].color; }));
        };
        var kl = cellKind(l, 'old');
        var kr = cellKind(r, 'new');
        out.push(html`<tr class="diffnote-split-row" key=${hi + ':' + pi} data-diffnote-threads=${ids.length ? ids.join(' ') : undefined}>
          <td class=${'diffnote-line__gutter-old diffnote-cell--' + kl + (shownL ? ' diffnote-gutter--commented' : '')} style=${shownL ? bars(idsL) : undefined}>${l && l.o != null ? l.o : ''}</td>
          <td class=${'diffnote-line__content diffnote-cell--' + kl}>${l && html`<code dangerouslySetInnerHTML=${{ __html: l.h }}></code>`}</td>
          <td class=${'diffnote-line__gutter-new diffnote-cell--' + kr + (shownR ? ' diffnote-gutter--commented' : '')} style=${shownR ? bars(idsR) : undefined}>${r && r.n != null ? r.n : ''}</td>
          <td class=${'diffnote-line__content diffnote-cell--' + kr}>${r && html`<code dangerouslySetInnerHTML=${{ __html: r.h }}></code>`}</td>
        </tr>`);
        // The cards of the pair: those of its new-side line, then its old-side line.
        var cards = lib.cardsOfRow(after, r || {});
        if (l && l !== r) cards = cards.concat(lib.cardsOfRow(after, { o: l.o }));
        cards.forEach(function (id) {
          out.push(html`<tr class="diffnote-thread-row" key=${'c' + id}><td colspan="4"><${Card} rev=${ctx.rev} thread=${ctx.byId[id]} placement=${ctx.placements[id]} /></td></tr>`);
        });
      });
    });
    return html`<div class="diffnote-diff-scroll"><table class="diffnote-diff diffnote-diff--split">
      <colgroup><col class="diffnote-col-gutter" /><col /><col class="diffnote-col-gutter" /><col /></colgroup>
      <tbody>${out}</tbody>
    </table></div>`;
  }

  // A file: its own threads, its diff (drawn when it is first opened), and
  // the threads that could not be placed in it.
  function File(props) {
    var file = props.file;
    var ctx = props.ctx;
    var mine = lib.threadsOfFile(ctx.order, ctx.placements, file.path);
    var fileThreads = mine.filter(function (id) { return ctx.placements[id].kind === 'file'; });
    var unplaced = mine.filter(function (id) { return ctx.placements[id].kind === 'unplaced'; });
    var startsOpen = mine.length > 0;
    var _ = useState(startsOpen);
    var opened = _[0];
    var setOpened = _[1];
    var missing = file.status === 'context' && file.hunks.length === 0;
    return html`<section class="diffnote-file" id=${'r' + ctx.rev + '-file-' + htmlId(file.path)}>
      <details open=${startsOpen} onToggle=${function (e) { if (e.target.open && !opened) setOpened(true); }}>
        <summary>
          <h2>${file.path}${file.status === 'binary' ? ' (バイナリ)' : ''}${file.status === 'renamed' ? ' (名前変更)' : ''}</h2>
          <button type="button" class="diffnote-copy" data-diffnote-copy=${file.path} title="パスをコピー">コピー</button>
        </summary>
        ${fileThreads.map(function (id) { return html`<${Card} key=${id} rev=${ctx.rev} thread=${ctx.byId[id]} placement=${ctx.placements[id]} />`; })}
        ${missing && html`<p class="diffnote-file__missing">このファイルは指定したdiffに含まれていません(コメント作成時点と異なるdiffを指定している可能性があります)。</p>`}
        ${opened && file.hunks.length > 0 && (ctx.layout === 'split' ? html`<${SplitTable} file=${file} ctx=${ctx} />` : html`<${DiffTable} file=${file} ctx=${ctx} />`)}
        ${unplaced.length > 0 && html`<section class="diffnote-outdated">
          <h3>未配置のコメント</h3>
          ${unplaced.map(function (id) {
            var p = ctx.placements[id];
            return html`<div class="diffnote-outdated__entry" key=${id}>
              ${p.was.length > 0 && html`<pre class="diffnote-outdated__snippet">${p.was.join('\n') + '\n'}</pre>`}
              <${Card} rev=${ctx.rev} thread=${ctx.byId[id]} placement=${p} />
            </div>`;
          })}
        </section>`}
      </details>
    </section>`;
  }

  function FileList(props) {
    var ctx = props.ctx;
    return html`<details class="diffnote-side" open>
      <summary>ファイル</summary>
      <nav class="diffnote-filelist"><ul>
        ${ctx.revision.files.map(function (f) {
          var n = lib.threadsOfFile(ctx.order, ctx.placements, f.path).length;
          return html`<li key=${f.path}><a href=${'#r' + ctx.rev + '-file-' + htmlId(f.path)}>${f.path}</a>${n > 0 && html` <span class="diffnote-badge">${n}</span>`}</li>`;
        })}
      </ul></nav>
    </details>`;
  }

  function ThreadList(props) {
    var ctx = props.ctx;
    var open = ctx.model.threads.filter(function (t) { return !t.resolved; }).length;
    return html`<details class="diffnote-side" open>
      <summary>スレッド <span class="diffnote-badge" title="未解決 / 全部">${open} / ${ctx.model.threads.length}</span></summary>
      <nav class="diffnote-threadlist"><ol>
        ${ctx.order.map(function (id) {
          var t = ctx.byId[id];
          var p = ctx.placements[id];
          var color = p && p.kind === 'line' ? lib.color(p.color) : '#8b949e';
          return html`<li key=${id} class=${t.resolved ? 'is-resolved' : ''}>
            <a href=${'#r' + ctx.rev + '-thread-' + id} data-diffnote-jump=${id} title=${lib.location(p) || '差分全体'}>
              <span class="diffnote-thread__swatch" style=${'background:' + color}></span><span class="diffnote-threadlist__where">${lib.shortLocation(p)}</span>${t.resolved && html`<span class="diffnote-threadlist__state">解決済み</span>`}<span class="diffnote-threadlist__preview">${lib.preview(t.comments[0].html)}</span>
            </a>
          </li>`;
        })}
      </ol></nav>
    </details>`;
  }

  // One revision: the side lists and the files.
  function Revision(props) {
    var model = props.model;
    var rev = props.index;
    var revision = model.revisions[rev];
    var byId = useMemo(function () {
      var m = {};
      model.threads.forEach(function (t) { m[t.id] = t; });
      return m;
    }, [model]);
    // The threads in the order they were written (their ids sort by time).
    var order = useMemo(function () { return model.threads.map(function (t) { return t.id; }); }, [model]);
    var ctx = {
      model: model, rev: rev, revision: revision, byId: byId, order: order,
      placements: revision.placements, hideResolved: props.hideResolved, layout: props.layout,
    };
    var globals = order.filter(function (id) { return revision.placements[id].kind === 'global'; });
    var listOrder = { model: model, rev: rev, revision: revision, byId: byId, order: revision.order, placements: revision.placements };

    // The file list marks the files that are on screen.
    useEffect(function () {
      if (!('IntersectionObserver' in window)) return undefined;
      var links = {};
      document.querySelectorAll('#rev-' + rev + ' .diffnote-filelist a').forEach(function (a) {
        links[(a.getAttribute('href') || '').slice(1)] = a;
      });
      var io = new IntersectionObserver(function (entries) {
        entries.forEach(function (en) {
          var a = links[en.target.id];
          if (a) a.classList.toggle('is-visible', en.isIntersecting);
        });
      }, { rootMargin: '-48px 0px -55% 0px' });
      document.querySelectorAll('#rev-' + rev + ' .diffnote-file').forEach(function (f) { io.observe(f); });
      return function () { io.disconnect(); };
    }, [rev]);

    return html`<section class="diffnote-revision is-current" id=${'rev-' + rev} data-diffnote-revision=${rev}>
      <h2 class="diffnote-revision__title">${revision.label}</h2>
      <aside class="diffnote-sidebar">
        <${FileList} ctx=${listOrder} />
        ${model.threads.length > 0 && html`<${ThreadList} ctx=${listOrder} />`}
      </aside>
      ${globals.length > 0 && html`<section class="diffnote-global-comments">
        ${globals.map(function (id) { return html`<${Card} key=${id} rev=${rev} thread=${byId[id]} placement=${revision.placements[id]} />`; })}
      </section>`}
      ${revision.files.map(function (f) { return html`<${File} key=${rev + ':' + f.path} file=${f} ctx=${ctx} />`; })}
    </section>`;
  }

  // Whether the window is wide enough for two columns of code.
  function useWide() {
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

  function revisionFromHash(model) {
    var m = /^#rev-(\d+)$/.exec(location.hash);
    if (m && +m[1] < model.revisions.length) return +m[1];
    return model.revisions.length - 1;
  }

  function App(props) {
    var model = props.model;
    var _c = useState(revisionFromHash(model));
    var current = _c[0];
    var setCurrent = _c[1];
    // Resolved threads are hidden unless that was turned off before.
    var _h = useState(kept('diffnote-hide-resolved', '1') !== '0');
    var hide = _h[0];
    var setHide = _h[1];
    var counts = lib.counts(model.threads);
    // Side by side, if chosen and there is room for two columns.
    var _l = useState(kept('diffnote-layout', 'unified') === 'split' ? 'split' : 'unified');
    var chosen = _l[0];
    var setChosen = _l[1];
    var wide = useWide();
    var layout = chosen === 'split' && wide ? 'split' : 'unified';

    // At once (not after the next paint): the style that hides cards hangs on it.
    useLayoutEffect(function () {
      document.body.classList.toggle('diffnote-hide-resolved', hide);
      D.interact.reset();
    }, [hide, current, layout]);

    return html`<article class="diffnote-review">
      <div class="diffnote-topbar">
        <header class="diffnote-summary">
          <h1>${model.title || DEFAULT_TITLE}</h1>
          <p>スレッド ${counts.all} 件(解決済み ${counts.resolved} 件)</p>
        </header>
        ${model.revisions.length > 1 && html`<nav class="diffnote-revisions"><ul>
          ${model.revisions.map(function (r, i) {
            return html`<li key=${i}><a href=${'#rev-' + i} data-diffnote-revision-link=${i} class=${i === current ? 'is-current' : ''}
              onClick=${function (e) { e.preventDefault(); setCurrent(i); }}>${r.label}</a></li>`;
          })}
        </ul></nav>`}
        ${wide && html`<div class="diffnote-layout" role="group" aria-label="差分の表示">
          ${[['unified', '統合'], ['split', '横並び']].map(function (o) {
            return html`<button type="button" key=${o[0]} data-diffnote-layout=${o[0]} class=${'diffnote-layout__button' + (o[0] === layout ? ' is-current' : '')}
              onClick=${function () { keep('diffnote-layout', o[0]); setChosen(o[0]); }}>${o[1]}</button>`;
          })}
        </div>`}
        ${counts.resolved > 0 && html`<label class="diffnote-toggle">
          <input type="checkbox" data-diffnote-hide-resolved checked=${hide}
            onChange=${function (e) { keep('diffnote-hide-resolved', e.target.checked ? '1' : '0'); setHide(e.target.checked); }} />
          解決済みを隠す<span class="diffnote-toggle__count" data-diffnote-resolved-count>${'(' + counts.resolved + ')'}</span>
        </label>`}
      </div>
      <${Revision} key=${current} model=${model} index=${current} hideResolved=${hide} layout=${layout} />
    </article>`;
  }

  D.start = function () {
    var model = JSON.parse(document.getElementById('diffnote-data').textContent);
    D.interact.install();
    render(html`<${App} model=${model} />`, document.getElementById('app'));
  };
})(window.Diffnote);
