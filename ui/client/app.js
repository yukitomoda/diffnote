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
  var useRef = preactHooks.useRef;
  var useContext = preactHooks.useContext;
  var html = htm.bind(h);

  // What the page can do to the review (`null` on an exported page): reply to
  // a thread, resolve or reopen it. Set by the App, read by the cards.
  var ActionsContext = preact.createContext(null);

  // Choosing lines and writing a new thread (`null` on an exported page).
  var ComposeContext = preact.createContext(null);

  // Files opened to look at (`null` on an exported page).
  var OpenedContext = preact.createContext(null);

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

  // The reply box and the resolve button of a card on the served page. A
  // reply shows at once as a faded comment and is put right by the answer; if it
  // fails the words stay in the box, with what went wrong.
  function Actions(props) {
    var t = props.thread;
    var actions = props.actions;
    var _t = useState('');
    var text = _t[0];
    var setText = _t[1];
    var _p = useState(null);
    var pending = _p[0];
    var setPending = _p[1];
    var _e = useState(null);
    var error = _e[0];
    var setError = _e[1];

    function send() {
      var body = text.trim();
      if (!body || pending !== null) return;
      setPending(body);
      setError(null);
      actions.reply(t.id, body).then(function (res) {
        setPending(null);
        if (res.ok) setText('');
        else setError(res.error || '保存できませんでした');
      });
    }
    function toggle() {
      setError(null);
      actions.setResolved(t.id, !t.resolved).then(function (res) {
        if (!res.ok) setError(res.error || '保存できませんでした');
      });
    }
    var action = t.resolved ? 'reopen' : 'resolve';
    return html`${pending !== null && html`<article class="diffnote-comment is-pending">
        <p class="diffnote-comment__author">保存中…</p>
        <div class="diffnote-comment__body">${pending}</div>
      </article>`}
      <div class="diffnote-thread__actions">
        <form class="diffnote-reply" data-diffnote-thread=${t.id} onSubmit=${function (e) { e.preventDefault(); send(); }}>
          <textarea rows="2" placeholder="返信を書く(Ctrl+Enter で送信)" value=${text} disabled=${pending !== null}
            onInput=${function (e) { setText(e.target.value); }}
            onKeyDown=${function (e) { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send(); } }}></textarea>
          <div class="diffnote-reply__buttons">
            <button type="submit" class="diffnote-button diffnote-button--primary">返信</button>
            <button type="button" class="diffnote-button" data-diffnote-action=${action} data-diffnote-thread=${t.id} onClick=${toggle}>${t.resolved ? '再開する' : '解決にする'}</button>
          </div>
          ${error && html`<p class="diffnote-error">${error}</p>`}
        </form>
      </div>`;
  }

  // One thread as a card.
  function Card(props) {
    var t = props.thread;
    var p = props.placement;
    var actions = useContext(ActionsContext);
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
      ${actions && html`<${Actions} thread=${t} actions=${actions} />`}
    </details>`;
  }

  // The box a new thread is written in: on chosen lines, on a file, or on the
  // whole review. What is written is kept while the choice changes.
  function Composer(props) {
    var c = useContext(ComposeContext);
    var box = useRef(null);
    useEffect(function () { box.current.focus(); }, []);
    var send = function () { c.send(props.request); };
    return html`<div>
      <form class="diffnote-compose" data-diffnote-scope=${props.scope} style=${c.pending ? 'display:none' : undefined}
        onSubmit=${function (e) { e.preventDefault(); send(); }}>
        <div class="diffnote-compose__where">${props.where}</div>
        <textarea ref=${box} rows="3" placeholder="コメントを書く(Ctrl+Enter で送信)" value=${c.draft}
          onInput=${function (e) { c.setDraft(e.target.value); }}
          onKeyDown=${function (e) { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send(); } }}></textarea>
        <div class="diffnote-reply__buttons">
          <button type="submit" class="diffnote-button diffnote-button--primary">コメントする</button>
          <button type="button" class="diffnote-button" data-diffnote-cancel onClick=${c.close}>キャンセル</button>
        </div>
        ${c.error && html`<p class="diffnote-error">${c.error}</p>`}
      </form>
      ${c.pending && html`<article class="diffnote-comment is-pending">
        <p class="diffnote-comment__author">保存中…</p>
        <div class="diffnote-comment__body">${c.draft}</div>
      </article>`}
    </div>`;
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
    var compose = useContext(ComposeContext);
    var flat = useMemo(function () { return lib.flatRows(file); }, [file]);
    var sel = compose && compose.sel && compose.sel.rev === ctx.rev && compose.sel.path === file.path ? compose.sel : null;
    var lo = sel ? Math.min(sel.anchor, sel.to) : -1;
    var hi_ = sel ? Math.max(sel.anchor, sel.to) : -1;
    var flatIndex = 0;
    var out = [];
    file.hunks.forEach(function (hunk, hi) {
      if (!file.opened) out.push(html`<tr class="diffnote-hunk-header" key=${'h' + hi}><td colspan="3">${hunk.header}</td></tr>`);
      hunk.rows.forEach(function (row, ri) {
        var idx = flatIndex++;
        var picked = idx >= lo && idx <= hi_;
        var ids = lib.covering(cover, row);
        var resolvedOnly = ctx.hideResolved && ids.length > 0 && ids.every(function (id) { return ctx.byId[id].resolved; });
        var cls =
          'diffnote-line--' + (row.k === 'c' ? 'context' : row.k === 'a' ? 'added' : 'removed') +
          (ids.length ? ' diffnote-line--commented' : '') +
          (resolvedOnly ? ' diffnote-line--resolved-only' : '') +
          (picked ? ' diffnote-select' + (idx === lo ? ' diffnote-select-first' : '') + (idx === hi_ ? ' diffnote-select-last' : '') : '');
        var colors = ids.map(function (id) { return ctx.placements[id].color; });
        var begin = compose && function (e) {
          if (e.button !== 0) return;
          e.preventDefault();
          compose.begin(ctx.rev, file.path, idx, e.shiftKey);
        };
        out.push(html`<tr
          class=${cls}
          key=${hi + ':' + ri}
          data-diffnote-old=${row.o != null ? row.o : undefined}
          data-diffnote-new=${row.n != null ? row.n : undefined}
          onMouseOver=${compose ? function () { compose.extend(idx); } : undefined}
          data-diffnote-threads=${ids.length ? ids.join(' ') : undefined}
          style=${ids.length ? '--diffnote-bars: ' + lib.bars(colors) : undefined}
        >
          <td class="diffnote-line__gutter-old" onMouseDown=${begin}>${row.o != null ? row.o : ''}</td>
          <td class="diffnote-line__gutter-new" onMouseDown=${begin}>${row.n != null ? row.n : ''}</td>
          <td class="diffnote-line__content"><code dangerouslySetInnerHTML=${{ __html: row.h }}></code></td>
        </tr>`);
        if (sel && !compose.selecting && idx === hi_) {
          var c = lib.counters(flat, sel.anchor, sel.to);
          out.push(html`<tr class="diffnote-composer-row" key="compose"><td colspan="3">
            <${Composer} scope="lines" where=${lib.chosenLocation(file.path, c)}
              request=${{ revision: ctx.rev, file: file.path, base: c.base, head: c.head }} />
          </td></tr>`);
        }
        lib.cardsOfRow(after, row).forEach(function (id) {
          out.push(html`<tr class="diffnote-thread-row" key=${'c' + id}><td colspan="3"><${Card} rev=${ctx.rev} thread=${ctx.byId[id]} placement=${ctx.placements[id]} /></td></tr>`);
        });
      });
    });
    return html`<div class="diffnote-diff-scroll"><table class="diffnote-diff" data-diffnote-file=${file.path}><tbody>${out}</tbody></table></div>`;
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
    // Lines are chosen on one side: the cells of that side, from the first row
    // chosen to the last.
    var compose = useContext(ComposeContext);
    var flat = useMemo(function () { return lib.flatRows(file); }, [file]);
    var indexOf = useMemo(function () {
      var m = new Map();
      flat.forEach(function (f, i) { m.set(f.row, i); });
      return m;
    }, [flat]);
    var sel = compose && compose.sel && compose.sel.rev === ctx.rev && compose.sel.path === file.path && compose.sel.side ? compose.sel : null;
    var lo = sel ? Math.min(sel.anchor, sel.to) : -1;
    var hi_ = sel ? Math.max(sel.anchor, sel.to) : -1;
    var idxOf = function (row) { return row ? indexOf.get(row) : undefined; };
    var pickedCell = function (row, side) {
      if (!sel || sel.side !== side || !row) return '';
      var i = idxOf(row);
      var has = side === 'old' ? row.o != null : row.n != null;
      if (!has || i < lo || i > hi_) return '';
      return ' is-picked' + (i === lo ? ' is-picked-first' : '') + (i === hi_ ? ' is-picked-last' : '');
    };
    var begin = function (row, side) {
      return compose && row && function (e) {
        if (e.button !== 0) return;
        e.preventDefault();
        compose.begin(ctx.rev, file.path, idxOf(row), e.shiftKey, side);
      };
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
        var pl = pickedCell(l, 'old');
        var pr = pickedCell(r, 'new');
        out.push(html`<tr class="diffnote-split-row" key=${hi + ':' + pi} data-diffnote-threads=${ids.length ? ids.join(' ') : undefined}
          onMouseOver=${compose ? function () { compose.extend(function (side) { return side === 'old' ? idxOf(l) : idxOf(r); }); } : undefined}>
          <td class=${'diffnote-line__gutter-old diffnote-cell--' + kl + (shownL ? ' diffnote-gutter--commented' : '') + pl} style=${shownL ? bars(idsL) : undefined}
            data-diffnote-old=${l && l.o != null ? l.o : undefined} onMouseDown=${begin(l, 'old')}>${l && l.o != null ? l.o : ''}</td>
          <td class=${'diffnote-line__content diffnote-cell--' + kl + pl}>${l && html`<code dangerouslySetInnerHTML=${{ __html: l.h }}></code>`}</td>
          <td class=${'diffnote-line__gutter-new diffnote-cell--' + kr + (shownR ? ' diffnote-gutter--commented' : '') + pr} style=${shownR ? bars(idsR) : undefined}
            data-diffnote-new=${r && r.n != null ? r.n : undefined} onMouseDown=${begin(r, 'new')}>${r && r.n != null ? r.n : ''}</td>
          <td class=${'diffnote-line__content diffnote-cell--' + kr + pr}>${r && html`<code dangerouslySetInnerHTML=${{ __html: r.h }}></code>`}</td>
        </tr>`);
        // The box for the choice: under the pair that has its last row.
        var last = sel && !compose.selecting ? flat[hi_].row : null;
        if (last && (l === last || r === last)) {
          var c = lib.counters(flat, sel.anchor, sel.to, sel.side);
          out.push(html`<tr class="diffnote-composer-row" key="compose"><td colspan="4">
            <${Composer} scope="lines" where=${lib.chosenLocation(file.path, c)}
              request=${{ revision: ctx.rev, file: file.path, base: c.base, head: c.head }} />
          </td></tr>`);
        }
        // The cards of the pair: those of its new-side line, then its old-side line.
        var cards = lib.cardsOfRow(after, r || {});
        if (l && l !== r) cards = cards.concat(lib.cardsOfRow(after, { o: l.o }));
        cards.forEach(function (id) {
          out.push(html`<tr class="diffnote-thread-row" key=${'c' + id}><td colspan="4"><${Card} rev=${ctx.rev} thread=${ctx.byId[id]} placement=${ctx.placements[id]} /></td></tr>`);
        });
      });
    });
    return html`<div class="diffnote-diff-scroll"><table class="diffnote-diff diffnote-diff--split" data-diffnote-file=${file.path}>
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
    var startsOpen = mine.length > 0 || !!file.opened;
    var files = useContext(OpenedContext);
    var _ = useState(startsOpen);
    var opened = _[0];
    var setOpened = _[1];
    var missing = file.status === 'context' && file.hunks.length === 0;
    var compose = useContext(ComposeContext);
    var details = useRef(null);
    var composing = compose && compose.scope && compose.scope.kind === 'file' && compose.scope.rev === ctx.rev && compose.scope.path === file.path;
    return html`<section class="diffnote-file" id=${'r' + ctx.rev + '-file-' + htmlId(file.path)} data-diffnote-file=${file.path}>
      <details ref=${details} open=${startsOpen} onToggle=${function (e) { if (e.target.open && !opened) setOpened(true); }}>
        <summary>
          <h2>${file.path}${file.status === 'binary' ? ' (バイナリ)' : ''}${file.status === 'renamed' ? ' (名前変更)' : ''}</h2>
          <button type="button" class="diffnote-copy" data-diffnote-copy=${file.path} title="パスをコピー">コピー</button>
          ${file.opened && files && html`<button type="button" class="diffnote-mini" data-diffnote-close title="この表示を閉じる(記録には残りません)"
            onClick=${function (e) {
              e.preventDefault();
              e.stopPropagation();
              if (compose && compose.scope && compose.scope.path === file.path) compose.close();
              if (compose && compose.sel && compose.sel.path === file.path) compose.close();
              files.close(ctx.rev, file.path);
            }}>閉じる</button>`}
          ${compose && (file.opened || file.status !== 'context') && html`<button type="button" class="diffnote-mini" data-diffnote-add="file" title="このファイルにコメントする"
            onClick=${function (e) {
              e.preventDefault();
              e.stopPropagation();
              details.current.open = true;
              setOpened(true);
              compose.openScope('file', ctx.rev, file.path);
            }}>コメント</button>`}
        </summary>
        ${composing && html`<div class="diffnote-compose-wrap"><${Composer} scope="file" where=${file.path + ' へのコメント'} request=${{ scope: 'file', revision: ctx.rev, file: file.path }} /></div>`}
        ${fileThreads.map(function (id) { return html`<${Card} key=${id} rev=${ctx.rev} thread=${ctx.byId[id]} placement=${ctx.placements[id]} />`; })}
        ${missing && html`<p class="diffnote-file__missing">このファイルは指定したdiffに含まれていません(コメント作成時点と異なるdiffを指定している可能性があります)。</p>`}
        ${opened && file.hunks.length > 0 && (ctx.layout === 'split' ? html`<${SplitTable} file=${file} ctx=${ctx} />` : html`<${DiffTable} file=${file} ctx=${ctx} />`)}
        ${opened && file.opened && file.next && html`<div class="diffnote-more-row"><button type="button" class="diffnote-button" data-diffnote-more
          onClick=${function (e) { e.target.disabled = true; files.more(ctx.rev, file.path).then(function () { e.target.disabled = false; }); }}>続きを表示(${file.next}〜 / 全 ${file.total} 行)</button></div>`}
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
          // The threads that are shown: resolved ones don't count while hidden.
          var n = lib.threadsOfFile(ctx.order, ctx.placements, f.path).filter(function (id) {
            return !(ctx.hideResolved && ctx.byId[id].resolved);
          }).length;
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

  // The entries of a directory of the files that could be opened (read from
  // the server when shown: a directory of thousands costs nothing until then).
  function TreeList(props) {
    var _ = useState(null);
    var data = _[0];
    var setData = _[1];
    useEffect(function () {
      var stale = false;
      setData(null);
      D.api.get('/api/files/' + props.rev + '/tree?json=1&dir=' + encodeURIComponent(props.dir) + '&q=' + encodeURIComponent(props.query)).then(function (res) {
        if (!stale) setData(res);
      });
      return function () { stale = true; };
    }, [props.rev, props.dir, props.query]);
    if (!data) return html`<p class="diffnote-tree__empty">読み込み中…</p>`;
    if (!data.ok) return html`<p class="diffnote-tree__empty">${data.error || '読み込めませんでした'}</p>`;
    return html`<${preact.Fragment}>
      ${data.message && html`<p class="diffnote-tree__empty">${data.message}</p>`}
      ${data.entries.length > 0 && html`<ul class="diffnote-tree__list">
        ${data.entries.map(function (e) {
          return e.kind === 'dir'
            ? html`<li key=${e.path}><${TreeDir} rev=${props.rev} entry=${e} onOpen=${props.onOpen} /></li>`
            : html`<li key=${e.path}><button type="button" class="diffnote-tree__file" data-diffnote-open=${e.path} title=${e.path}
                onClick=${function () { props.onOpen(e.path); }}>${e.name}</button></li>`;
        })}
      </ul>`}
      ${data.note && html`<p class="diffnote-tree__empty">${data.note}</p>`}
      ${data.more > 0 && html`<p class="diffnote-tree__empty">ほか ${data.more} 件(検索で絞り込んでください)</p>`}
    <//>`;
  }

  function TreeDir(props) {
    var _ = useState(false);
    var shown = _[0];
    var setShown = _[1];
    var e = props.entry;
    return html`<details class="diffnote-tree__dir" data-diffnote-dir=${e.path} onToggle=${function (ev) { if (ev.target.open) setShown(true); }}>
      <summary>${e.name}/ <span class="diffnote-tree__count">${e.count}</span></summary>
      ${shown && html`<${TreeList} rev=${props.rev} dir=${e.path} query="" onOpen=${props.onOpen} />`}
    </details>`;
  }

  // "Other files": what the review has (or, next to the repository, the commit
  // has) that the diff doesn't show. Opening one shows it, records nothing.
  function Tree(props) {
    var files = useContext(OpenedContext);
    var _s = useState(false);
    var shown = _s[0];
    var setShown = _s[1];
    var _q = useState('');
    var typed = _q[0];
    var setTyped = _q[1];
    var _d = useState('');
    var query = _d[0];
    var setQuery = _d[1];
    var _e = useState('');
    var error = _e[0];
    var setError = _e[1];
    useEffect(function () {
      var t = setTimeout(function () { setQuery(typed); }, 250);
      return function () { clearTimeout(t); };
    }, [typed]);
    var open = function (path) {
      setError('');
      files.open(props.rev, path).then(function (res) { if (!res.ok) setError(res.error || '開けませんでした'); });
    };
    return html`<details class="diffnote-side diffnote-side--quiet" data-diffnote-tree onToggle=${function (e) { if (e.target === e.currentTarget && e.target.open) setShown(true); }}>
      <summary>その他のファイル</summary>
      <div class="diffnote-tree">
        <input type="search" class="diffnote-tree__search" placeholder="ファイルを検索" aria-label="ファイルを検索" value=${typed}
          onInput=${function (e) { setTyped(e.target.value); }} />
        <div data-diffnote-tree-list>${shown && html`<${TreeList} rev=${props.rev} dir="" query=${query} onOpen=${open} />`}</div>
        ${error && html`<p class="diffnote-error">${error}</p>`}
      </div>
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
    // The files opened to look at come after the diff's; one that a thread has
    // since brought in keeps the lines that were opened.
    var opened = useContext(OpenedContext);
    var files = useMemo(function () {
      var mine = (opened && opened.byRev[rev]) || [];
      var seen = {};
      var merged = revision.files.map(function (f) {
        var o = mine.filter(function (x) { return x.path === f.path; })[0];
        if (!o) return f;
        seen[f.path] = true;
        return Object.assign({}, f, { hunks: o.hunks, opened: true, next: o.next, total: o.total });
      });
      mine.forEach(function (o) {
        if (!seen[o.path]) merged.push({ path: o.path, old_path: null, status: 'context', hunks: o.hunks, opened: true, next: o.next, total: o.total });
      });
      return merged;
    }, [revision, opened && opened.byRev[rev]]);
    var ctx = {
      model: model, rev: rev, revision: revision, byId: byId, order: order,
      placements: revision.placements, hideResolved: props.hideResolved, layout: props.layout,
    };
    var globals = order.filter(function (id) { return revision.placements[id].kind === 'global'; });
    var listOrder = { model: model, rev: rev, revision: Object.assign({}, revision, { files: files }), hideResolved: props.hideResolved, byId: byId, order: revision.order, placements: revision.placements };

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
        ${opened && html`<${Tree} rev=${rev} />`}
      </aside>
      ${(globals.length > 0 || props.compose) && html`<section class="diffnote-global-comments" data-diffnote-global>
        ${props.compose && html`<div class="diffnote-add"><button type="button" class="diffnote-button" data-diffnote-add="global"
          onClick=${function () { props.compose.openScope('global', rev); }}>レビュー全体にコメントする</button></div>`}
        ${props.compose && props.compose.scope && props.compose.scope.kind === 'global' && props.compose.scope.rev === rev && html`<div class="diffnote-compose-wrap"><${Composer} scope="global" where="レビュー全体へのコメント" request=${{ scope: 'global', revision: rev }} /></div>`}
        ${globals.map(function (id) { return html`<${Card} key=${id} rev=${rev} thread=${byId[id]} placement=${revision.placements[id]} />`; })}
      </section>`}
      ${files.map(function (f) { return html`<${File} key=${rev + ':' + f.path} file=${f} ctx=${ctx} />`; })}
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

  // The model, and (on the served page) the changes that can be made to it.
  // A change is shown at once and put right by the server's answer. The answer
  // says how many events the review has and how many the change added: if the
  // review had changed under the page (a `diffnote edit`, another tab), those
  // don't add up and the whole model is fetched again -- as it is whenever the
  // window is looked at again and the review has more events than the page's.
  function useReview(initial) {
    var _m = useState(initial);
    var model = _m[0];
    var setModel = _m[1];
    var ref = useRef(model);
    ref.current = model;

    var reloadModel = preactHooks.useCallback(function () {
      return D.api.get('/api/model').then(function (res) {
        if (res.ok) setModel(res.model);
        return res;
      });
    }, []);

    var actions = useMemo(function () {
      if (!initial.interactive) return null;
      var replace = function (thread) {
        return function (cur) {
          return Object.assign({}, cur, {
            threads: cur.threads.map(function (t) { return t.id === thread.id ? thread : t; }),
          });
        };
      };
      // What a change's answer does to the page.
      var settle = function (res) {
        if (res.events - res.appended !== ref.current.events) {
          reloadModel();
          return;
        }
        setModel(function (cur) {
          return Object.assign({}, replace(res.thread_data)(cur), { events: res.events });
        });
      };
      return {
        reply: function (id, text) {
          return D.api.post('/api/threads/' + id + '/replies', { body: text }).then(function (res) {
            if (res.ok) settle(res);
            return res;
          });
        },
        // A new thread: the answer has the whole model, with it placed.
        create: function (request) {
          return D.api.post('/api/threads', Object.assign({ model: true }, request)).then(function (res) {
            if (res.ok) setModel(res.model);
            return res;
          });
        },
        setResolved: function (id, resolved) {
          var before = ref.current.threads.filter(function (t) { return t.id === id; })[0];
          setModel(replace(Object.assign({}, before, { resolved: resolved })));
          return D.api.post('/api/threads/' + id + '/' + (resolved ? 'resolve' : 'reopen')).then(function (res) {
            if (res.ok) settle(res);
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
        D.api.get('/api/version').then(function (res) {
          if (res.ok && res.events !== ref.current.events) reloadModel();
        });
      };
      window.addEventListener('focus', check);
      document.addEventListener('visibilitychange', check);
      return function () {
        window.removeEventListener('focus', check);
        document.removeEventListener('visibilitychange', check);
      };
    }, [initial.interactive]);

    return { model: model, actions: actions };
  }

  // Lines being chosen (pressing a line number, dragging, Shift+click), or a
  // box open for a file or the review, and what is written in it. `null` when
  // the page can't change the review.
  function useCompose(actions, current) {
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
          D.interact.reset();
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
            else setError(res.error || '保存できませんでした');
          });
        },
      };
    }, [actions, sel, selecting, scope, draft, pending, error]);
  }

  // The files opened to look at, per revision, and the lines of each read so
  // far. Kept here, not in the model: nothing is recorded by opening one.
  function useOpened(interactive) {
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
          return D.api.get('/api/files/' + rev + '/open?json=1&path=' + encodeURIComponent(path)).then(function (res) {
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
          return D.api.get('/api/files/' + rev + '/more?json=1&path=' + encodeURIComponent(path) + '&from=' + file.next).then(function (res) {
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

  function App(props) {
    var review = useReview(props.model);
    var openedFiles = useOpened(props.model.interactive);
    var model = review.model;
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
    // What was chosen or written belongs to the revision and layout it was in.
    var compose = useCompose(review.actions, current + ':' + layout);

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
        ${(counts.resolved > 0 || model.interactive) && html`<label class="diffnote-toggle">
          <input type="checkbox" data-diffnote-hide-resolved checked=${hide}
            onChange=${function (e) { keep('diffnote-hide-resolved', e.target.checked ? '1' : '0'); setHide(e.target.checked); }} />
          解決済みを隠す<span class="diffnote-toggle__count" data-diffnote-resolved-count>${'(' + counts.resolved + ')'}</span>
        </label>`}
        ${model.interactive && html`<a class="diffnote-button" data-diffnote-export href="/export" title="今の内容を、誰でも開ける HTML として保存します">エクスポート</a>`}
        ${model.interactive && html`<button type="button" class="diffnote-button diffnote-topbar__quit" data-diffnote-shutdown title="サーバーを止めます"
          onClick=${function () { D.api.post('/api/shutdown').then(function () { document.body.innerHTML = '<p style="padding:24px;font:14px sans-serif">終了しました。このタブは閉じてかまいません。</p>'; }); }}>終了</button>`}
      </div>
      <${ActionsContext.Provider} value=${review.actions}>
        <${ComposeContext.Provider} value=${compose}>
          <${OpenedContext.Provider} value=${openedFiles}>
            <${Revision} key=${current} model=${model} index=${current} hideResolved=${hide} layout=${layout} compose=${compose} />
          <//>
        <//>
      <//>
    </article>`;
  }

  D.start = function () {
    var model = JSON.parse(document.getElementById('diffnote-data').textContent);
    if (model.interactive) document.body.setAttribute('data-diffnote-api', '1');
    D.interact.install();
    render(html`<${App} model=${model} />`, document.getElementById('app'));
  };
})(window.Diffnote);
