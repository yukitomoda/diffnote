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
  // The files marked as looked at: `is(file)` and `toggle(file)` (a file that has
  // become another one since is not marked any more).
  var ViewedContext = preact.createContext(null);
  // Places in a comment's text that name lines of a file: `has(path)` and
  // `go(path, start, end)`.
  var LinksContext = preact.createContext(null);
  // How the diff is shown, and the ways to change it (the view menu).
  var ViewContext = preact.createContext(null);

  var DEFAULT_TITLE = 'diffnote レビュー';
  var BINARY_CHANGES = { added: '追加', deleted: '削除', renamed: '名前変更', modified: '変更' };

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
    var attach = useAttach(text, setText);
    var field = useRef(null);

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
          <div class="diffnote-attach-bar">${attach.picker(function () { return field.current; })}</div>
          <textarea ref=${field} rows="2" placeholder="返信を書く(Ctrl+Enter で送信)" value=${text} disabled=${pending !== null}
            ...${attach.handlers}
            onInput=${function (e) { setText(e.target.value); }}
            onKeyDown=${function (e) { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send(); } }}></textarea>
          ${attach.note}
          <div class="diffnote-reply__buttons">
            <button type="submit" class="diffnote-button diffnote-button--primary">返信</button>
            <button type="button" class="diffnote-button" data-diffnote-action=${action} data-diffnote-thread=${t.id} onClick=${toggle}>${t.resolved ? '再開する' : '解決にする'}</button>
          </div>
          ${error && html`<p class="diffnote-error">${error}</p>`}
        </form>
      </div>`;
  }

  // A value that can be changed on the spot: its display (the children) and a
  // button to edit it, which turns into a box to write the new value in.
  // Pictures for a box that a comment is written in: pasted (a screenshot),
  // dropped, or chosen. Each goes to the server, and what stands for it in the
  // text is put where the cursor was. Only on the served page.
  function useAttach(text, setText) {
    var _s = useState(null);
    var status = _s[0];
    var setStatus = _s[1];
    var latest = useRef(text);
    latest.current = text;
    var links = useContext(LinksContext);
    var send = function (files, field) {
      var list = Array.prototype.slice.call(files || []);
      if (!D.api || list.length === 0) return false;
      var limit = links && links.limit;
      // Too big: said here, before anything is sent.
      var big = limit && list.filter(function (f) { return f.size > limit; })[0];
      if (big) {
        setStatus({ failed: true, text: '「' + (big.name || 'ファイル') + '」(' + lib.formatSize(big.size) + ')は、添付できる大きさ(' + lib.formatSize(limit) + ')を超えています' });
        return true;
      }
      var from = field.selectionStart;
      var to = field.selectionEnd;
      setStatus({ busy: true, text: '送っています…' });
      var snippets = [];
      var last = null;
      var images = 0;
      var chain = list.reduce(function (p, file) {
        return p.then(function () {
          var isImage = /^image\//.test(file.type);
          var name = file.name || (isImage ? 'image' : 'file');
          return (isImage ? D.api.upload(file) : D.api.uploadFile(file, name)).then(function (res) {
            if (!res.ok) throw new Error(res.error || '添付できませんでした');
            if (isImage) images++;
            snippets.push(isImage ? lib.imageMarkdown(res.id) : lib.fileMarkdown(name, res.id));
            last = { res: res, name: name, isImage: isImage };
          });
        });
      }, Promise.resolve());
      chain.then(function () {
        var put = lib.insertAt(latest.current, from, to, snippets.join('\n') + '\n');
        setText(put.text);
        var what = list.length > 1 ? list.length + ' 件を添付しました' : last.isImage ? '画像を追加しました' : '「' + last.name + '」を添付しました';
        setStatus({ text: what + '(' + lib.formatSize(last.res.size) + ')。バンドルの大きさ: ' + lib.formatSize(last.res.bundle_size) + (last.res.size > 5 * 1024 * 1024 ? '。大きなファイルです' : '') });
      }, function (err) {
        setStatus({ failed: true, text: err.message });
      });
      return true;
    };
    return {
      status: status,
      handlers: D.api ? {
        onPaste: function (e) {
          if (send(e.clipboardData && e.clipboardData.files, e.currentTarget)) e.preventDefault();
        },
        onDrop: function (e) {
          e.currentTarget.classList.remove('is-dropping');
          if (send(e.dataTransfer && e.dataTransfer.files, e.currentTarget)) e.preventDefault();
        },
        // Over a box with a file: it says it can take it.
        onDragOver: function (e) {
          if (e.dataTransfer && Array.prototype.indexOf.call(e.dataTransfer.types || [], 'Files') >= 0) {
            e.preventDefault();
            e.currentTarget.classList.add('is-dropping');
          }
        },
        onDragLeave: function (e) { e.currentTarget.classList.remove('is-dropping'); },
      } : {},
      // The button that opens the file chooser, and the note under the box.
      picker: function (field) {
        if (!D.api) return null;
        return html`<label class="diffnote-attach" title="ファイルや画像を添付します(貼り付けや、ドラッグ&ドロップでも追加できます)">📎 添付
          <input type="file" multiple data-diffnote-attach
            onChange=${function (e) { var f = field(); if (f) send(e.target.files, f); e.target.value = ''; }} /></label>`;
      },
      note: status && html`<p class=${'diffnote-attach__status' + (status.failed ? ' is-failed' : '')} data-diffnote-attach-status role="status">${status.text}</p>`,
    };
  }

  function InlineEdit(props) {
    var _e = useState(false);
    var editing = _e[0];
    var setEditing = _e[1];
    var _t = useState('');
    var text = _t[0];
    var setText = _t[1];
    var _b = useState(false);
    var busy = _b[0];
    var setBusy = _b[1];
    var _r = useState('');
    var error = _r[0];
    var setError = _r[1];
    var box = useRef(null);
    useEffect(function () { if (editing && box.current) { box.current.focus(); box.current.select(); } }, [editing]);
    var save = function (e) {
      e.preventDefault();
      if (busy) return;
      setBusy(true);
      setError('');
      props.onSave(text).then(function (res) {
        setBusy(false);
        if (res.ok) setEditing(false);
        else setError(res.error || '保存できませんでした');
      });
    };
    if (!editing) {
      return html`<span class="diffnote-inline">${props.children}<button type="button" class="diffnote-inline__edit"
        data-diffnote-inline=${props.name} title=${props.label} aria-label=${props.label}
        onClick=${function () { setText(props.value); setError(''); setEditing(true); }}>✎</button></span>`;
    }
    return html`<span class="diffnote-inline">${props.children}<form class=${'diffnote-inline diffnote-inline--editing diffnote-inline--' + props.name} onSubmit=${save}>
      <input ref=${box} type="text" value=${text} maxlength=${props.max} placeholder=${props.placeholder} aria-label=${props.label}
        data-diffnote-inline-input=${props.name}
        onInput=${function (e) { setText(e.target.value); }}
        onKeyDown=${function (e) { if (e.key === 'Escape') { e.stopPropagation(); setEditing(false); } }} />
      <button type="submit" class="diffnote-button diffnote-button--primary" disabled=${busy}>保存</button>
      <button type="button" class="diffnote-button" onClick=${function () { setEditing(false); }}>取消</button>
      ${error && html`<span class="diffnote-error" role="alert">${error}</span>`}
    </form></span>`;
  }

  // One comment. One added since the server started has buttons to edit it and
  // to take it out (the first comment of a thread takes the whole thread out).
  function Comment(props) {
    var c = props.comment;
    var actions = props.actions;
    var links = useContext(LinksContext);
    var mine = !!actions && actions.editable.has(c.id);
    var _e = useState(false);
    var editing = _e[0];
    var setEditing = _e[1];
    var _t = useState('');
    var text = _t[0];
    var setText = _t[1];
    var _b = useState(false);
    var busy = _b[0];
    var setBusy = _b[1];
    var _r = useState('');
    var error = _r[0];
    var setError = _r[1];
    var attach = useAttach(text, setText);
    var field = useRef(null);
    var save = function () {
      if (!text.trim() || busy) return;
      setBusy(true);
      setError('');
      actions.edit(c.id, text).then(function (res) {
        setBusy(false);
        if (res.ok) setEditing(false);
        else setError(res.error || '保存できませんでした');
      });
    };
    var remove = function () {
      var what = props.first && props.replies > 0 ? 'このスレッドを、返信も含めて削除しますか?' : props.first ? 'このスレッドを削除しますか?' : 'この返信を削除しますか?';
      if (!window.confirm(what)) return;
      setBusy(true);
      setError('');
      actions.remove(c.id).then(function (res) {
        setBusy(false);
        if (!res.ok) setError(res.error || '削除できませんでした');
      });
    };
    return html`<article class="diffnote-comment" data-diffnote-comment=${c.id}>
      <p class="diffnote-comment__author">${c.author}<${Time} at=${c.at} />${mine && !editing && html`<span class="diffnote-comment__tools">
        <button type="button" class="diffnote-mini" data-diffnote-edit disabled=${busy}
          onClick=${function () { setText(c.body || ''); setError(''); setEditing(true); }}>編集</button>
        <button type="button" class="diffnote-mini" data-diffnote-delete disabled=${busy} onClick=${remove}>削除</button>
      </span>`}</p>
      ${editing
        ? html`<form class="diffnote-compose" data-diffnote-edit-form onSubmit=${function (e) { e.preventDefault(); save(); }}>
            <div class="diffnote-attach-bar">${attach.picker(function () { return field.current; })}</div>
            <textarea ref=${field} rows="3" value=${text} ...${attach.handlers} onInput=${function (e) { setText(e.target.value); }}
              onKeyDown=${function (e) {
                if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); save(); }
                if (e.key === 'Escape') { e.stopPropagation(); setEditing(false); }
              }}></textarea>
            ${attach.note}
            <div class="diffnote-reply__buttons">
              <button type="submit" class="diffnote-button diffnote-button--primary" disabled=${busy}>保存</button>
              <button type="button" class="diffnote-button" onClick=${function () { setEditing(false); }}>キャンセル</button>
            </div>
            ${error && html`<p class="diffnote-error">${error}</p>`}
          </form>`
        : html`<div class="diffnote-comment__body">${markdown(c.doc, links)}</div>${error && html`<p class="diffnote-error">${error}</p>`}`}
    </article>`;
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
        html`<button type="button" class="diffnote-copy" data-diffnote-copy=${loc + '@' + (props.rev + 1)} title="ファイルパスと行(とリビジョン)のリンクをコピー">コピー</button>`}
      </summary>
      ${absent && absent.was.length > 0 && html`<pre class="diffnote-deleted__snippet">${absent.was.join('\n') + '\n'}</pre>`}
      ${t.comments.map(function (c, i) {
        return html`<${Comment} key=${c.id} comment=${c} actions=${actions} first=${i === 0} replies=${t.comments.length - 1} />`;
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
    var attach = useAttach(c.draft, c.setDraft);
    return html`<div>
      <form class="diffnote-compose" data-diffnote-scope=${props.scope} style=${c.pending ? 'display:none' : undefined}
        onSubmit=${function (e) { e.preventDefault(); send(); }}>
        <div class="diffnote-compose__head"><div class="diffnote-compose__where">${props.where}</div>${props.copy && html`<button type="button" class="diffnote-copy" data-diffnote-copy=${props.copy} title="この範囲へのリンク(リビジョンつき)をコピー">コピー</button>`}<span class="diffnote-attach-bar">${attach.picker(function () { return box.current; })}</span></div>
        <textarea ref=${box} rows="3" placeholder="コメントを書く(Ctrl+Enter で送信)" value=${c.draft} ...${attach.handlers}
          onInput=${function (e) { c.setDraft(e.target.value); }}
          onKeyDown=${function (e) { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send(); } }}></textarea>
        ${attach.note}
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

  // 「表示」: how the diff is shown, in a menu at the top right of the diff.
  function ViewMenu() {
    var v = useContext(ViewContext);
    var _o = useState(false);
    var open = _o[0];
    var setOpen = _o[1];
    var box = useRef(null);
    useEffect(function () {
      if (!open) return undefined;
      var away = function (e) { if (box.current && !box.current.contains(e.target)) setOpen(false); };
      var key = function (e) { if (e.key === 'Escape') setOpen(false); };
      document.addEventListener('mousedown', away);
      document.addEventListener('keydown', key);
      return function () {
        document.removeEventListener('mousedown', away);
        document.removeEventListener('keydown', key);
      };
    }, [open]);
    // (The panel is in the page while it is closed, only not shown.)
    return html`<div class="diffnote-viewmenu" ref=${box}>
      <button type="button" class="diffnote-button" data-diffnote-view-menu aria-haspopup="true" aria-expanded=${open}
        onClick=${function () { setOpen(!open); }}>⚙ 表示 ▾</button>
      <div class="diffnote-viewmenu__panel" hidden=${!open} data-diffnote-view-panel>
        ${v.wide && html`<div class="diffnote-layout" role="group" aria-label="レイアウト">
          <p class="diffnote-viewmenu__head">レイアウト</p>
          ${[['unified', '統合'], ['split', '横並び']].map(function (o) {
            return html`<button type="button" key=${o[0]} class=${'diffnote-viewmenu__item diffnote-layout__button' + (o[0] === v.layout ? ' is-current' : '')} data-diffnote-layout=${o[0]}
              onClick=${function () { v.setLayout(o[0]); }}>${o[1]}</button>`;
          })}
          <hr />
        </div>`}
        <label class=${'diffnote-viewmenu__item' + (v.ignoreSpace ? ' is-current' : '')} title="行の中の空白だけが違う変更を、変更なしとして表示します">
          <input type="checkbox" data-diffnote-ignore-space checked=${v.ignoreSpace} onChange=${function (e) { v.toggleSpace(e.target.checked); }} />空白の違いを無視
        </label>
        ${(v.resolved > 0 || v.interactive) && html`<label class=${'diffnote-viewmenu__item' + (v.hide ? ' is-current' : '')}>
          <input type="checkbox" data-diffnote-hide-resolved checked=${v.hide} onChange=${function (e) { v.setHide(e.target.checked); }} />解決済みを隠す<span class="diffnote-toggle__count" data-diffnote-resolved-count>${'(' + v.resolved + ')'}</span>
        </label>`}
      </div>
    </div>`;
  }

  // The name comments are written under, at the foot of the side, like the
  // user who is signed in.
  function UserChip(props) {
    var name = props.name;
    var initial = Array.from(name.trim())[0] || '?';
    return html`<div class="diffnote-user" data-diffnote-user>
      <span class="diffnote-user__avatar" aria-hidden="true">${initial.toUpperCase()}</span>
      <${InlineEdit} name="author" value=${name} max="100" label="作者名を変える(この起動の間だけ)" placeholder="作者名"
        onSave=${props.onSave}><strong data-diffnote-author>${name}</strong><//>
    </div>`;
  }

  // 「終了」: the way to finish, big; and, behind the arrow, the way not to
  // keep what was done in this session (asked again before it is done).
  function QuitButton() {
    var _o = useState(false);
    var open = _o[0];
    var setOpen = _o[1];
    var _s = useState(false);
    var sure = _s[0];
    var setSure = _s[1];
    var _e = useState(null);
    var error = _e[0];
    var setError = _e[1];
    var box = useRef(null);
    useEffect(function () {
      if (!open) return undefined;
      var away = function (e) { if (box.current && !box.current.contains(e.target)) { setOpen(false); setSure(false); } };
      var key = function (e) { if (e.key === 'Escape') { setOpen(false); setSure(false); } };
      document.addEventListener('mousedown', away);
      document.addEventListener('keydown', key);
      return function () {
        document.removeEventListener('mousedown', away);
        document.removeEventListener('keydown', key);
      };
    }, [open]);
    var quit = function (discard) {
      D.api.post('/api/shutdown', discard ? { discard: true } : undefined).then(function (res) {
        if (res.ok) stopped(res.summary);
        else setError(res.error || '終了できませんでした');
      });
    };
    return html`<span class="diffnote-quit" ref=${box}>
      <button type="button" class="diffnote-quit__main" data-diffnote-shutdown title="今の内容で保存して、サーバーを止めます"
        onClick=${function () { quit(false); }}>終了</button><button type="button" class="diffnote-quit__more" data-diffnote-quit-more aria-label="ほかの終了のしかた" aria-expanded=${open}
        onClick=${function () { setOpen(!open); setSure(false); }}>▾</button>
      ${open && html`<div class="diffnote-quit__menu" data-diffnote-quit-menu>
        ${!sure
          ? html`<button type="button" class="diffnote-quit__item" data-diffnote-discard onClick=${function () { setSure(true); }}>保存せずに終了…</button>`
          : html`<p>この起動で行ったことを、すべて取り消して終了します(取り込んだ差分やタイトルも含みます)。バンドルは起動する前の状態に戻り、この起動で作ったバンドルなら、削除されます。</p>
            <button type="button" class="diffnote-quit__danger" data-diffnote-discard-confirm onClick=${function () { quit(true); }}>破棄して終了</button>
            <button type="button" class="diffnote-quit__cancel" onClick=${function () { setSure(false); setOpen(false); }}>やめる</button>`}
        ${error && html`<p class="diffnote-error">${error}</p>`}
      </div>`}
    </span>`;
  }

  // What the tab shows once the server has stopped.
  function stopped(summary) {
    preact.render(null, document.getElementById('app'));
    var make = function (tag, cls, text) {
      var e = document.createElement(tag);
      e.className = cls;
      if (text != null) e.textContent = text;
      return e;
    };
    var card = make('div', 'diffnote-farewell__card');
    var discarded = !!(summary && summary.discarded);
    card.appendChild(make('div', 'diffnote-farewell__mark', discarded ? '↩' : '✓'));
    card.appendChild(make('h1', 'diffnote-farewell__title', discarded ? '保存せずに終了しました' : '終了しました'));
    if (discarded) {
      var kept = make('dl', 'diffnote-farewell__list');
      kept.appendChild(make('dt', '', '今回の変更'));
      kept.appendChild(make('dd', '', '破棄しました'));
      kept.appendChild(make('dt', '', '保存先'));
      kept.appendChild(make('dd', '', summary.path + (summary.removed ? '(この起動で作ったので、削除しました)' : '(起動する前の内容のままです)')));
      card.appendChild(kept);
    } else if (summary) {
      var list = make('dl', 'diffnote-farewell__list');
      var row = function (label, value) {
        list.appendChild(make('dt', '', label));
        list.appendChild(make('dd', '', value));
      };
      row('今回の変更', summary.changes);
      row('保存先', summary.path);
      if (summary.threads != null) row('いまの内容', 'スレッド ' + summary.threads + ' 件・コメント ' + summary.comments + ' 件');
      card.appendChild(list);
    }
    card.appendChild(make('p', 'diffnote-farewell__note', 'このタブは閉じてかまいません。'));
    var page = make('div', 'diffnote-farewell');
    page.appendChild(card);
    document.body.replaceChildren(page);
  }

  // A comment: the nodes of its Markdown (see `src/html/markdown.rs`) as
  // elements. Only what is known is drawn, so nothing a comment says can be
  // anything but text; a link goes only to http, https or mailto.
  var SAFE_LINK = /^(https?:|mailto:)/i;
  function markdown(nodes, links, inLink) {
    return (nodes || []).map(function (n, i) {
      if (typeof n === 'string') {
        if (!links || inLink) return n;
        // `src/a.ts:10-13` in the text goes to those lines.
        return lib.lineRefs(n, links.has, links.revisions).map(function (piece, j) {
          if (typeof piece === 'string') return piece;
          var where = links.current === (piece.rev == null ? links.current : piece.rev - 1) ? 'この行へ移ります' : 'リビジョン #' + piece.rev + ' の行へ移ります';
          if (piece.rev != null && piece.rev < links.revisions) where += '(最新のリビジョンではありません)';
          return html`<a key=${j} href="#" class="diffnote-lineref" data-diffnote-lineref=${piece.path + ':' + (piece.side === 'old' ? 'L' : '') + piece.start + '-' + piece.end} title=${where}
            onClick=${function (e) { e.preventDefault(); links.go(piece); }}>${piece.text}</a>`;
        });
      }
      var kids = markdown(n.c, links, inLink || n.t === 'a');
      switch (n.t) {
        case 'p': return h('p', { key: i }, kids);
        case 'h': return h('h' + Math.min(Math.max(n.l || 1, 1), 6), { key: i }, kids);
        case 'quote': return h('blockquote', { key: i }, kids);
        case 'ul': return h('ul', { key: i }, kids);
        case 'ol': return h('ol', { key: i, start: n.start }, kids);
        case 'li': return h('li', { key: i }, kids);
        case 'pre': return h('pre', { key: i }, h('code', null, n.s || ''));
        case 'hr': return h('hr', { key: i });
        case 'em': return h('em', { key: i }, kids);
        case 'strong': return h('strong', { key: i }, kids);
        case 'del': return h('del', { key: i }, kids);
        case 'table': return h('div', { key: i, class: 'diffnote-table' }, h('table', null, (n.c || []).map(function (row, r) {
          var head = row.t === 'thead';
          return h(head ? 'thead' : 'tbody', { key: r }, h('tr', null, (row.c || []).map(function (cell, j) {
            var al = { l: 'left', c: 'center', r: 'right' }[(n.al || [])[j]];
            return h(head ? 'th' : 'td', { key: j, style: al ? 'text-align:' + al : undefined }, markdown(cell.c, links, inLink));
          })));
        })));
        case 'image': {
          // An image of the review, drawn only as an <img>: nothing else is loaded.
          var src = links && links.image ? links.image(n.id) : '';
          return src ? h('img', { key: i, class: 'diffnote-image', src: src, alt: n.alt || '' }) : h('span', { key: i }, n.alt || '[画像]');
        }
        case 'file': {
          // Another file of the review: only ever to be saved.
          var name = lib.plainText(n.c).trim() || 'file';
          var href = links && links.file ? links.file(n.id, name) : '';
          return href ? h('a', { key: i, class: 'diffnote-attachment', href: href, download: name, rel: 'noopener' }, '📎 ', kids) : h('span', { key: i }, kids);
        }
        case 'code': return h('code', { key: i }, n.s || '');
        case 'br': return h('br', { key: i });
        case 'a':
          return n.href && SAFE_LINK.test(n.href)
            ? h('a', { key: i, href: n.href, target: '_blank', rel: 'noopener noreferrer' }, kids)
            : h('span', { key: i }, kids);
        default: return h('span', { key: i }, kids);
      }
    });
  }

  // A line of code: its pieces of text, each with the kind of thing it is (a
  // plain piece is just its text). The colors are the style's (`.tok-*`).
  function tokens(pieces, changed) {
    return lib.markPieces(pieces, changed).map(function (p, i) {
      var cls = (p[0] ? 'tok tok-' + p[0] : '') + (p[2] ? (p[0] ? ' ' : '') + 'diffnote-word' : '');
      return cls ? html`<span key=${i} class=${cls}>${p[1]}</span>` : p[1];
    });
  }

  // What stands for the lines a diff leaves out: buttons to show some of them
  // (next to the hunk above, next to the hunk below) or all.
  function Expander(props) {
    var m = props.marker;
    var _ = useState(false);
    var busy = _[0];
    var setBusy = _[1];
    var go = function (where) {
      if (busy) return;
      setBusy(true);
      props.expand(m.gap, where).then(function () { setBusy(false); });
    };
    var step = lib.EXPAND_STEP;
    // Can they be had? Carried by the page (an export), or asked of the server.
    var can = m.x && (m.embedded || !!D.api);
    if (!can) {
      return html`<div class="diffnote-expand"><span class="diffnote-expand__label">${m.left} 行省略</span>${m.x && !m.embedded && !D.api && html`<span class="diffnote-expand__note">(この HTML には含まれていません)</span>`}</div>`;
    }
    return html`<div class="diffnote-expand">
      ${m.left > step && m.prev && html`<button type="button" class="diffnote-expand__button" data-diffnote-expand="top" disabled=${busy} onClick=${function () { go('top'); }}>↓ ${step} 行</button>`}
      ${m.left > step && m.next && html`<button type="button" class="diffnote-expand__button" data-diffnote-expand="bottom" disabled=${busy} onClick=${function () { go('bottom'); }}>↑ ${step} 行</button>`}
      <button type="button" class="diffnote-expand__button diffnote-expand__all" data-diffnote-expand="all" disabled=${busy} onClick=${function () { go('all'); }}>${m.left} 行をすべて表示</button>
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
      if (hunk.marker) {
        out.push(html`<tr class="diffnote-expand-row" key=${'g' + hi}><td colspan="3"><${Expander} marker=${hunk.marker} expand=${props.expand} /></td></tr>`);
        return;
      }
      if (!file.opened && !hunk.quiet) out.push(html`<tr class="diffnote-hunk-header" key=${'h' + hi}><td colspan="3">${hunk.header}</td></tr>`);
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
        // The bars are of the threads that are shown (not those hidden as resolved).
        var colors = lib.shownIds(ids, ctx.byId, ctx.hideResolved).map(function (id) { return ctx.placements[id].color; });
        var begin = compose && function (e) {
          if (e.button !== 0) return;
          // Against another revision, a removed line is not in this one.
          if (ctx.compare && row.k === 'd') return;
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
          <td class="diffnote-line__content"><code>${tokens(row.t, row.w)}</code></td>
        </tr>`);
        if (sel && !compose.selecting && idx === hi_) {
          var c = lib.counters(flat, sel.anchor, sel.to);
          out.push(html`<tr class="diffnote-composer-row" key="compose"><td colspan="3">
            <${Composer} scope="lines" where=${lib.chosenLocation(file.path, c)} copy=${lib.chosenLocation(file.path, c) + '@' + (ctx.rev + 1)}
              request=${ctx.compare ? { revision: ctx.rev, file: file.path, head: c.head } : { revision: ctx.rev, file: file.path, base: c.base, head: c.head }} />
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
      // (Against another revision, only the new side is this revision's.)
      if (ctx.compare && side === 'old') return undefined;
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
      if (hunk.marker) {
        out.push(html`<tr class="diffnote-expand-row" key=${'g' + hi}><td colspan="4"><${Expander} marker=${hunk.marker} expand=${props.expand} /></td></tr>`);
        return;
      }
      if (!hunk.quiet) out.push(html`<tr class="diffnote-hunk-header" key=${'h' + hi}><td colspan="4">${hunk.header}</td></tr>`);
      lib.pairRows(hunk.rows).forEach(function (pair, pi) {
        var l = pair.left;
        var r = pair.right;
        var idsL = l && l.o != null ? cover.old[l.o] || [] : [];
        var idsR = r && r.n != null ? cover.new[r.n] || [] : [];
        var ids = idsL.concat(idsR.filter(function (id) { return idsL.indexOf(id) < 0; }));
        var shownL = idsL.length > 0 && !hidden(idsL);
        var shownR = idsR.length > 0 && !hidden(idsR);
        var bars = function (side) {
          return '--diffnote-bars: ' + lib.bars(lib.shownIds(side, ctx.byId, ctx.hideResolved).map(function (id) { return ctx.placements[id].color; }));
        };
        var kl = cellKind(l, 'old');
        var kr = cellKind(r, 'new');
        var pl = pickedCell(l, 'old');
        var pr = pickedCell(r, 'new');
        out.push(html`<tr class="diffnote-split-row" key=${hi + ':' + pi} data-diffnote-threads=${ids.length ? ids.join(' ') : undefined}
          onMouseOver=${compose ? function () { compose.extend(function (side) { return side === 'old' ? idxOf(l) : idxOf(r); }); } : undefined}>
          <td class=${'diffnote-line__gutter-old diffnote-cell--' + kl + (shownL ? ' diffnote-gutter--commented' : '') + pl} style=${shownL ? bars(idsL) : undefined}
            data-diffnote-old=${l && l.o != null ? l.o : undefined} onMouseDown=${begin(l, 'old')}>${l && l.o != null ? l.o : ''}</td>
          <td class=${'diffnote-line__content diffnote-cell--' + kl + pl}>${l && html`<code>${tokens(l.t, l.w)}</code>`}</td>
          <td class=${'diffnote-line__gutter-new diffnote-cell--' + kr + (shownR ? ' diffnote-gutter--commented' : '') + pr} style=${shownR ? bars(idsR) : undefined}
            data-diffnote-new=${r && r.n != null ? r.n : undefined} onMouseDown=${begin(r, 'new')}>${r && r.n != null ? r.n : ''}</td>
          <td class=${'diffnote-line__content diffnote-cell--' + kr + pr}>${r && html`<code>${tokens(r.t, r.w)}</code>`}</td>
        </tr>`);
        // The box for the choice: under the pair that has its last row.
        var last = sel && !compose.selecting ? flat[hi_].row : null;
        if (last && (l === last || r === last)) {
          var c = lib.counters(flat, sel.anchor, sel.to, sel.side);
          out.push(html`<tr class="diffnote-composer-row" key="compose"><td colspan="4">
            <${Composer} scope="lines" where=${lib.chosenLocation(file.path, c)} copy=${lib.chosenLocation(file.path, c) + '@' + (ctx.rev + 1)}
              request=${ctx.compare ? { revision: ctx.rev, file: file.path, head: c.head } : { revision: ctx.rev, file: file.path, base: c.base, head: c.head }} />
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
    // The lines of the diff's left-out places that have been shown (their pieces,
    // by new line number): kept by number, so they stay when the places change.
    var _g = useState({});
    var revealed = _g[0];
    var setRevealed = _g[1];
    var shown = useMemo(function () { return lib.shownFrom(file.gaps, revealed); }, [file.gaps, revealed]);
    var view = useMemo(function () { return lib.withGaps(ctx.ignoreSpace ? lib.withoutSpaceChanges(file) : file, shown); }, [file, shown, ctx.ignoreSpace]);
    var expand = function (gap, where) {
      var g = file.gaps[gap];
      var req = lib.expandRequest(g, shown[gap] || { top: [], bottom: [] }, where);
      if (!req) return Promise.resolve();
      // Rows of the file are counted by position: what was chosen is let go.
      if (compose && compose.sel && compose.sel.path === file.path) compose.close();
      var get = function (offset, count) {
        if (g.t) return Promise.resolve(g.t.slice(offset, offset + count));
        var part = function (from, left, acc) {
          var take = Math.min(left, 1000);
          return D.api.get('/api/files/' + ctx.rev + '/lines?path=' + encodeURIComponent(file.path) + '&from=' + (g.w + from) + '&count=' + take).then(function (res) {
            if (!res.ok || res.lines.length === 0) return acc;
            acc = acc.concat(res.lines);
            return left > take ? part(from + take, left - take, acc) : acc;
          });
        };
        return part(offset, count, []);
      };
      return get(req.offset, req.count).then(function (lines) {
        setRevealed(function (cur) {
          var all = Object.assign({}, cur);
          lines.forEach(function (pieces, i) { all[g.w + req.offset + i] = pieces; });
          return all;
        });
      });
    };
    var composing = compose && compose.scope && compose.scope.kind === 'file' && compose.scope.rev === ctx.rev && compose.scope.path === file.path;
    // A file that was added or deleted as a whole (a binary one too) is tinted.
    var kind = file.status === 'binary' ? file.change : file.status;
    var viewed = useContext(ViewedContext);
    // A file that was looked at is not shown at all (with its threads): the
    // list at the side says so, and takes it back.
    if (viewed && viewed.is(file)) return null;
    return html`<section class=${'diffnote-file' + (kind === 'added' || kind === 'deleted' ? ' diffnote-file--' + kind : '')} id=${'r' + ctx.rev + '-file-' + htmlId(file.path)} data-diffnote-file=${file.path}>
      <details ref=${details} open=${startsOpen} onToggle=${function (e) { if (e.target.open && !opened) setOpened(true); }}>
        <summary>
          ${viewed && html`<button type="button" class="diffnote-mini diffnote-mini--check" data-diffnote-viewed=${file.path} title="確認済みにして、表示をたたみます(左の一覧から戻せます)"
            onClick=${function (e) { e.preventDefault(); e.stopPropagation(); viewed.toggle(file); }}>✓ 確認済み</button>`}
          <h2>${file.path}${file.status === 'binary' ? ' (バイナリ' + (BINARY_CHANGES[file.change] ? '・' + BINARY_CHANGES[file.change] : '') + ')' : ''}${file.status === 'renamed' ? ' (名前変更)' : ''}</h2>
          <button type="button" class="diffnote-copy" data-diffnote-copy=${file.path} title="パスをコピー">コピー</button>
          ${file.opened && files && html`<button type="button" class="diffnote-mini" data-diffnote-close title="この表示を閉じる(記録には残りません)"
            onClick=${function (e) {
              e.preventDefault();
              e.stopPropagation();
              if (compose && compose.scope && compose.scope.path === file.path) compose.close();
              if (compose && compose.sel && compose.sel.path === file.path) compose.close();
              files.close(ctx.rev, file.path);
            }}>閉じる</button>`}
          ${compose && (file.opened || file.status !== 'context' || mine.length > 0) && html`<button type="button" class="diffnote-mini" data-diffnote-add="file" title="このファイルにコメントする"
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
        ${file.status === 'binary' && html`<p class="diffnote-file__binary" data-diffnote-binary>バイナリファイルのため、内容は表示しません。</p>`}
        ${opened && file.hunks.length > 0 && (ctx.layout === 'split' ? html`<${SplitTable} file=${view} ctx=${ctx} expand=${expand} />` : html`<${DiffTable} file=${view} ctx=${ctx} expand=${expand} />`)}
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
    var viewed = useContext(ViewedContext);
    // The files of the diff (not those opened to look at) are what is counted.
    var files = ctx.diffFiles;
    return html`<details class="diffnote-side" open>
      <summary>ファイル${viewed && files.length > 0 && html` <span class="diffnote-badge diffnote-badge--viewed" data-diffnote-viewed-count title="確認済みにしたファイル / ファイル数">✓ ${files.filter(viewed.is).length}/${files.length}</span>`}</summary>
      <nav class="diffnote-filelist"><ul>
        ${ctx.revision.files.map(function (f) {
          var done = !!(viewed && viewed.is(f));
          var ids = lib.threadsOfFile(ctx.order, ctx.placements, f.path);
          // The threads that are shown: resolved ones don't count while hidden.
          var n = ids.filter(function (id) {
            return !(ctx.hideResolved && ctx.byId[id].resolved);
          }).length;
          // What is left open in a file that was looked at.
          var open = ids.filter(function (id) { return !ctx.byId[id].resolved; }).length;
          return html`<li key=${f.path} class=${done ? 'is-viewed' : ''}>
            ${viewed && html`<button type="button" class="diffnote-check" data-diffnote-check=${f.path} aria-pressed=${done}
              title=${done ? '確認済みを解除して、表示に戻します' : '確認済みにします'} onClick=${function () { viewed.toggle(f); }}>${done ? '✓' : ''}</button>`}
            <a href=${'#r' + ctx.rev + '-file-' + htmlId(f.path)} data-diffnote-file-link=${f.path}
              onClick=${function (e) {
                // A file that was looked at comes back; one that was folded opens; and it is marked.
                e.preventDefault();
                if (viewed && viewed.is(f)) viewed.toggle(f);
                D.interact.showFile(ctx.rev, f.path);
              }}>${f.path}</a>
            ${done
              ? open > 0 && html`<span class="diffnote-badge" data-diffnote-open-count title=${'未解決のスレッドが ' + open + ' 件あります'}>${open}</span>`
              : n > 0 && html` <span class="diffnote-badge">${n}</span>`}
          </li>`;
        })}
      </ul></nav>
    </details>`;
  }

  function ThreadList(props) {
    var ctx = props.ctx;
    var viewed = useContext(ViewedContext);
    var open = ctx.model.threads.filter(function (t) { return !t.resolved; }).length;
    return html`<details class="diffnote-side" open>
      <summary>スレッド <span class="diffnote-badge" title="未解決 / 全部">${open} / ${ctx.model.threads.length}</span></summary>
      <nav class="diffnote-threadlist"><ol>
        ${ctx.order.map(function (id) {
          var t = ctx.byId[id];
          var p = ctx.placements[id];
          var color = p && p.kind === 'line' ? lib.color(p.color) : '#8b949e';
          return html`<li key=${id} class=${t.resolved ? 'is-resolved' : ''}>
            <a href=${'#r' + ctx.rev + '-thread-' + id} data-diffnote-jump=${id} title=${lib.location(p) || '差分全体'}
              onClick=${function () {
                // A thread of a file that was looked at brings the file back.
                var file = p && p.file && ctx.revision.files.filter(function (f) { return f.path === p.file; })[0];
                if (file && viewed && viewed.is(file)) {
                  viewed.toggle(file);
                  D.interact.jumpWhenShown('r' + ctx.rev + '-thread-' + id);
                }
              }}>
              <span class="diffnote-thread__swatch" style=${'background:' + color}></span><span class="diffnote-threadlist__where">${lib.shortLocation(p)}</span>${t.resolved && html`<span class="diffnote-threadlist__state">解決済み</span>`}<span class="diffnote-threadlist__preview">${lib.preview(t.comments[0].doc)}</span>
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
    // (Compared with an earlier revision instead of the base: another view of it.)
    var revision = props.override || model.revisions[rev];
    var byId = useMemo(function () {
      var m = {};
      model.threads.forEach(function (t) { m[t.id] = t; });
      return m;
    }, [model]);
    // The threads in the order they were written (their ids sort by time).
    // (A thread that the revision's data doesn't know yet, being written while it is
    // looked at against another, waits for that data to come again.)
    var order = useMemo(function () {
      return model.threads.map(function (t) { return t.id; }).filter(function (id) { return !!revision.placements[id]; });
    }, [model, revision]);
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
      placements: revision.placements, hideResolved: props.hideResolved, layout: props.layout, ignoreSpace: props.ignoreSpace, compare: !!props.override,
    };
    var globals = order.filter(function (id) { return revision.placements[id].kind === 'global'; });
    var viewed = useContext(ViewedContext);
    var viewedPaths = {};
    files.forEach(function (f) { if (viewed && viewed.is(f)) viewedPaths[f.path] = true; });
    var listOrder = { diffFiles: revision.files, viewedPaths: viewedPaths, model: model, rev: rev, revision: Object.assign({}, revision, { files: files }), hideResolved: props.hideResolved, byId: byId, order: revision.order, placements: revision.placements };

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
    }, [rev, Object.keys(viewedPaths).join('\n')]);

    return html`<section class="diffnote-revision is-current" id=${'rev-' + rev} data-diffnote-revision=${rev}>
      <h2 class="diffnote-revision__title">${revision.label}</h2>
      <aside class="diffnote-sidebar">
        <div class="diffnote-sidebar__lists">
          <${FileList} ctx=${listOrder} />
          ${model.threads.length > 0 && html`<${ThreadList} ctx=${listOrder} />`}
          ${opened && html`<${Tree} rev=${rev} />`}
        </div>
        ${props.author != null && props.onAuthor && html`<${UserChip} name=${props.author} onSave=${props.onAuthor} />`}
      </aside>
      <div class="diffnote-viewbar">
        ${props.compose && html`<div class="diffnote-add"><button type="button" class="diffnote-button" data-diffnote-add="global"
          onClick=${function () { props.compose.openScope('global', rev); }}>レビュー全体にコメントする</button></div>`}
        ${props.override && html`<p class="diffnote-compare-note" data-diffnote-compare-note tabindex="0" title=${props.overrideNote.tip} aria-label=${props.overrideNote.short + '。' + props.overrideNote.tip}>${props.overrideNote.short}<span class="diffnote-compare-note__icon" aria-hidden="true">⚠</span></p>`}
        <${ViewMenu} />
      </div>
      ${(globals.length > 0 || props.compose) && html`<section class="diffnote-global-comments" data-diffnote-global>
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
  // says what stamp the review had before the change: if that isn't the page's
  // (the review had changed under it: a `diffnote edit`, another tab), the whole
  // model is fetched again -- as it is whenever the window is looked at again
  // and the review's stamp is not the page's.
  function useReview(initial) {
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
        if (res.before !== ref.current.stamp) {
          reloadModel();
          return;
        }
        setModel(function (cur) {
          return Object.assign({}, replace(res.thread_data)(cur), { stamp: res.stamp, editable: res.editable });
        });
      };
      var latest = {};
      // A change whose answer is the whole model (it may take a thread away).
      var whole = function (res) {
        if (res.ok) setModel(res.model);
        return res;
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
          return D.api.post('/api/threads', request).then(whole);
        },
        // Comments added since the server started can be rewritten or taken out.
        edit: function (id, text) {
          return D.api.post('/api/comments/' + id + '/edit', { body: text }).then(whole);
        },
        remove: function (id) {
          return D.api.post('/api/comments/' + id + '/delete').then(whole);
        },
        // The review's title (the answer is the whole model), and the name comments
        // are written under for the rest of this session.
        setTitle: function (title) {
          return D.api.post('/api/title', { title: title }).then(whole);
        },
        // What was added to the target since the server started becomes a new
        // revision (the answer says what was done; the page keeps its place).
        refresh: function () {
          return D.api.post('/api/refresh').then(function (res) {
            if (res.ok) {
              setModel(res.model);
              setPending(false);
            }
            return res;
          });
        },
        // Whether differences that are only in white space are hidden: kept in the
        // review (the answer is the whole model).
        setIgnoreWhitespace: function (ignore) {
          return D.api.post('/api/whitespace', { ignore: ignore }).then(whole);
        },
        setAuthor: function (name) {
          return D.api.post('/api/author', { author: name }).then(function (res) {
            if (res.ok) setModel(function (cur) { return Object.assign({}, cur, { author: res.author }); });
            return res;
          });
        },
        setResolved: function (id, resolved) {
          var before = ref.current.threads.filter(function (t) { return t.id === id; })[0];
          setModel(replace(Object.assign({}, before, { resolved: resolved })));
          // Pressed again before the answer came: only the last answer says how
          // the thread is (an earlier one would flip it back for a moment).
          var n = (latest[id] = (latest[id] || 0) + 1);
          return D.api.post('/api/threads/' + id + '/' + (resolved ? 'resolve' : 'reopen')).then(function (res) {
            if (latest[id] !== n) {
              if (res.ok) setModel(function (cur) { return Object.assign({}, cur, { stamp: res.stamp, editable: res.editable }); });
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
        D.api.get('/api/version').then(function (res) {
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
    var full = useMemo(function () {
      if (!actions) return null;
      return Object.assign({}, actions, { editable: new Set(model.editable || []) });
    }, [actions, model.editable]);
    return { model: model, actions: full, pending: pending };
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
    // The revision is looked at against an earlier one (chosen at the base) instead
    // of against the base: the number of that one, or `null`. Only for looking.
    var _a = useState(null);
    var against = _a[0];
    var setAgainst = _a[1];
    var _k = useState(null);
    var cmp = _k[0];
    var setCmp = _k[1];
    useEffect(function () {
      if (against != null && against >= current) setAgainst(null);
    }, [current, against]);
    useEffect(function () {
      if (!review.actions || against == null || against >= current) { setCmp(null); return undefined; }
      var stale = false;
      D.api.get('/api/compare?rev=' + current + '&from=' + against).then(function (res) {
        if (stale) return;
        if (res.ok) setCmp({ rev: current, from: against, data: res.revision });
        else { setCmp(null); setAgainst(null); }
      });
      return function () { stale = true; };
    }, [against, current, model.stamp]);
    // What the page draws for the revision then: the same files, but a file that
    // is marked as looked at is the same one (its text is this revision's).
    var override = useMemo(function () {
      if (!cmp || cmp.rev !== current || cmp.from !== against) return null;
      var sigs = {};
      model.revisions[current].files.forEach(function (f) { sigs[f.path] = f.sig; });
      return Object.assign({}, cmp.data, {
        files: cmp.data.files.map(function (f) {
          return Object.prototype.hasOwnProperty.call(sigs, f.path) ? Object.assign({}, f, { sig: sigs[f.path] }) : f;
        }),
      });
    }, [cmp, current, against, model]);
    // The tab that is shown is kept in view when there are more than fit.
    var tabs = useRef(null);
    useLayoutEffect(function () {
      var nav = tabs.current;
      var here = nav && nav.querySelector('a.is-current');
      if (here) nav.scrollLeft = here.offsetLeft - (nav.clientWidth - here.offsetWidth) / 2;
    }, [current, model.revisions.length]);
    // The files marked as looked at, by path, with what the file was then.
    var _v = useState({});
    var seen = _v[0];
    var setSeen = _v[1];
    var viewed = useMemo(function () {
      var is = function (f) { return Object.prototype.hasOwnProperty.call(seen, f.path) && seen[f.path] === (f.sig || ''); };
      return {
        is: is,
        toggle: function (f) {
          setSeen(function (cur) {
            var next = Object.assign({}, cur);
            if (Object.prototype.hasOwnProperty.call(cur, f.path) && cur[f.path] === (f.sig || '')) delete next[f.path];
            else next[f.path] = f.sig || '';
            return next;
          });
        },
      };
    }, [seen]);
    var here = model.revisions[current] ? model.revisions[current].files : [];
    var links = useMemo(function () {
      // A path is a place if some revision has the file.
      var known = {};
      model.revisions.forEach(function (r) { r.files.forEach(function (f) { known[f.path] = true; }); });
      return {
        current: current,
        revisions: model.revisions.length,
        has: function (path) { return Object.prototype.hasOwnProperty.call(known, path); },
        // The most a file attached to a comment may weigh (the review's rule).
        limit: model.attachment_limit,
        // Where another attached file is: in the page, or at the server.
        file: function (id, name) {
          if (model.attachments && model.attachments[id]) return model.attachments[id];
          return model.interactive ? '/api/attachments/' + id + '?name=' + encodeURIComponent(name) : '';
        },
        // Where an image is: in the page (an exported one), or at the server.
        image: function (id) {
          if (model.images && model.images[id]) return model.images[id];
          return model.interactive ? '/api/images/' + id : '';
        },
        go: function (ref) {
          var index = ref.rev == null ? current : ref.rev - 1;
          if (index !== current) setCurrent(index);
          var file = model.revisions[index].files.filter(function (f) { return f.path === ref.path; })[0];
          // A file that was looked at is brought back to be shown.
          if (file && viewed.is(file)) viewed.toggle(file);
          D.interact.showLines(index, ref.path, ref.side, ref.start, ref.end);
        },
      };
    }, [model, viewed, current]);
    // Differences that are only in white space hidden: what the review says (and
    // a page that only shows it can change for itself).
    var _w = useState(!!model.ignore_whitespace);
    var localIgnore = _w[0];
    var setLocalIgnore = _w[1];
    var ignoreSpace = review.actions ? !!model.ignore_whitespace : localIgnore;
    var toggleSpace = function (on) {
      if (review.actions) review.actions.setIgnoreWhitespace(on);
      else setLocalIgnore(on);
    };
    // What pressing 「最新を取り込む」 did, said next to it.
    var _n = useState(null);
    var note = _n[0];
    var setNote = _n[1];
    var pull = function () {
      setNote({ text: '取り込んでいます…', busy: true });
      review.actions.refresh().then(function (res) {
        setNote({ text: res.ok ? res.message : (res.error || '取り込めませんでした'), failed: !res.ok });
        // What was taken in is what to look at now.
        if (res.ok && res.added) setCurrent(res.model.revisions.length - 1);
      });
    };
    // Side by side, if chosen and there is room for two columns. What was chosen
    // before is kept; without a choice the page starts side by side if the
    // window is wide (only when it opens: resizing the window doesn't change it).
    var _l = useState(function () {
      var stored = kept('diffnote-layout', '');
      if (stored === 'split' || stored === 'unified') return stored;
      return window.matchMedia('(min-width: 1200px)').matches ? 'split' : 'unified';
    });
    var chosen = _l[0];
    var setChosen = _l[1];
    var wide = useWide();
    var layout = chosen === 'split' && wide ? 'split' : 'unified';
    var viewOptions = {
      wide: wide, layout: layout, resolved: counts.resolved, interactive: !!model.interactive,
      hide: hide, ignoreSpace: ignoreSpace, toggleSpace: toggleSpace,
      setLayout: function (o) { keep('diffnote-layout', o); setChosen(o); },
      setHide: function (on) { keep('diffnote-hide-resolved', on ? '1' : '0'); setHide(on); },
    };
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
          <h1>${review.actions
            ? html`<${InlineEdit} name="title" value=${model.title || ''} max="200" label="タイトルを変える" placeholder="タイトル(空にすると、既定の見出しに戻ります)"
                onSave=${review.actions.setTitle}>${model.title || DEFAULT_TITLE}<//>`
            : model.title || DEFAULT_TITLE}</h1>
          ${model.base && html`<p data-diffnote-base class=${against != null ? 'is-changed' : ''} title=${against != null ? 'ベースの代わりに、このリビジョンと比べて表示しています(記録は変わりません)' : 'すべてのリビジョンは、これと比べた差分です'}>ベース: ${review.actions && current > 0
            ? html`<select class="diffnote-base__select" data-diffnote-base-select aria-label="比べる相手" value=${against == null ? '' : String(against)}
                onChange=${function (e) { setAgainst(e.target.value === '' ? null : +e.target.value); }}>
                <option value="">${model.base.kind === 'git' ? model.base.id : lib.formatTime(model.base.at)}</option>
                ${model.revisions.slice(0, current).map(function (r, i) { return html`<option key=${i} value=${String(i)}>${r.label}</option>`; })}
              </select>`
            : model.base.kind === 'git' ? html`<code>${model.base.id}</code>` : lib.formatTime(model.base.at)}</p>`}
        </header>
        ${model.revisions.length > 0 && html`<nav class="diffnote-revisions" ref=${tabs} onWheel=${function (e) {
          // The tabs scroll sideways (no bar is shown): the wheel does it too.
          if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) { e.currentTarget.scrollLeft += e.deltaY; e.preventDefault(); }
        }}><ul>
          ${model.revisions.map(function (r, i) {
            return html`<li key=${i}><a href=${'#rev-' + i} data-diffnote-revision-link=${i} class=${i === current ? 'is-current' : ''}
              onClick=${function (e) { e.preventDefault(); setCurrent(i); }}>${r.label}</a></li>`;
          })}
        </ul></nav>`}
        <div class="diffnote-topbar__actions">
        ${review.actions && model.refreshable && html`<span class="diffnote-pull">
          ${review.pending && html`<span class="diffnote-pull__pending" data-diffnote-pending role="status">新しいコミットがあります</span>`}
          <button type="button" class=${'diffnote-button' + (review.pending ? ' diffnote-button--primary' : '')} data-diffnote-pull disabled=${!!(note && note.busy)} onClick=${pull}
            title="起動したあとに増えたコミットなど、対象の新しい変更を、新しいリビジョンとして取り込み、それを表示します">最新を取り込む</button>
          ${note && html`<span class=${'diffnote-pull__note' + (note.failed ? ' is-failed' : '')} data-diffnote-pull-note role="status">${note.text}</span>`}
        </span>`}
        ${model.interactive && html`<a class="diffnote-button" data-diffnote-export href="/export" title="今の内容を、誰でも開ける HTML として保存します">エクスポート</a>`}
        ${model.interactive && html`<${QuitButton} />`}
        </div>
      </div>
      <${ViewContext.Provider} value=${viewOptions}>
      <${ViewedContext.Provider} value=${viewed}>
      <${LinksContext.Provider} value=${links}>
      <${ActionsContext.Provider} value=${review.actions}>
        <${ComposeContext.Provider} value=${compose}>
          <${OpenedContext.Provider} value=${openedFiles}>
            <${Revision} key=${current} model=${model} index=${current} hideResolved=${hide} layout=${layout} ignoreSpace=${ignoreSpace} compose=${compose} override=${override}
              overrideNote=${override ? {
                short: model.revisions[against].label.replace(/ \(.*$/, '') + ' .. ' + model.revisions[current].label.replace(/ \(.*$/, ''),
                tip: 'ベースの代わりに、選んだリビジョンと比べた差分を表示しています(表示だけの切り替えです)。コメントは、これまでどおり、ベースとの差分に付きます。比べた相手にだけある、削除された行には、コメントを付けられません。',
              } : null}
              author=${review.actions ? model.author : null} onAuthor=${review.actions && review.actions.setAuthor} />
          <//>
        <//>
      <//>
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
