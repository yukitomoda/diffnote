// Putting a picture or a file in a comment: the buttons, and what they upload.
import { useContext, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { transport } from '../transport.ts';
import { html } from '../html.ts';
import { LinksContext } from '../state/contexts.js';
import { EmojiButton } from './Reactions.js';

// Pictures for a box that a comment is written in: pasted (a screenshot),
// dropped, or chosen. Each goes to the server, and what stands for it in the
// text is put where the cursor was. Only on the served page.
export function useAttach(text, setText) {
  var _s = useState(null);
  var status = _s[0];
  var setStatus = _s[1];
  var latest = useRef(text);
  latest.current = text;
  var links = useContext(LinksContext);
  var send = function (files, field) {
    var list = Array.prototype.slice.call(files || []);
    if (!transport || list.length === 0) return false;
    var limit = links && links.limit;
    // Too big: said here, before anything is sent.
    var big = limit && list.filter(function (f) { return f.size > limit; })[0];
    if (big) {
      setStatus({
        failed: true,
        text: lib.mf('ui.attach.too_big', {
          name: big.name || lib.m('ui.attach.file_fallback'),
          size: lib.formatSize(big.size),
          limit: lib.formatSize(limit),
        }),
      });
      return true;
    }
    var from = field.selectionStart;
    var to = field.selectionEnd;
    setStatus({ busy: true, text: lib.m('ui.attach.sending') });
    var snippets = [];
    var last = null;
    var images = 0;
    var chain = list.reduce(function (p, file) {
      return p.then(function () {
        var isImage = /^image\//.test(file.type);
        var name = file.name || (isImage ? 'image' : 'file');
        return (isImage ? transport.upload(file) : transport.uploadFile(file, name)).then(function (res) {
          if (!res.ok) throw new Error(res.error || lib.m('ui.attach.failed'));
          if (isImage) images++;
          snippets.push(isImage ? lib.imageMarkdown(res.id) : lib.fileMarkdown(name, res.id));
          last = { res: res, name: name, isImage: isImage };
        });
      });
    }, Promise.resolve());
    chain.then(function () {
      var put = lib.insertAt(latest.current, from, to, snippets.join('\n') + '\n');
      setText(put.text);
      var what = list.length > 1
        ? lib.mf('ui.attach.done_multi', { count: String(list.length) })
        : last.isImage ? lib.m('ui.attach.done_image') : lib.mf('ui.attach.done_file', { name: last.name });
      var status = lib.mf('ui.attach.status_bundle_size', {
        size: lib.formatSize(last.res.size),
        bundle_size: lib.formatSize(last.res.bundle_size),
      });
      setStatus({ text: what + status + (last.res.size > 5 * 1024 * 1024 ? lib.m('ui.attach.big_file_note') : '') });
    }, function (err) {
      setStatus({ failed: true, text: err.message });
    });
    return true;
  };
  return {
    status: status,
    handlers: transport ? {
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
    // The buttons above the box (emoji, and the file chooser), and, below, the
    // note under it.
    picker: function (field) {
      if (!transport) return null;
      var pick = function (ch) {
        var box = field();
        if (!box) return;
        var put = lib.insertAt(latest.current, box.selectionStart, box.selectionEnd, ch);
        setText(put.text);
        setTimeout(function () { box.focus(); box.setSelectionRange(put.cursor, put.cursor); }, 0);
      };
      return html`<${EmojiButton} onPick=${pick} /><label class="diffnote-attach" title=${lib.m('ui.attach.picker_title')}>${lib.m('ui.attach.button_label')}
        <input type="file" multiple data-diffnote-attach
          onChange=${function (e) { var f = field(); if (f) send(e.target.files, f); e.target.value = ''; }} /></label>`;
    },
    note: status && html`<p class=${'diffnote-attach__status' + (status.failed ? ' is-failed' : '')} data-diffnote-attach-status role="status">${status.text}</p>`,
  };
}
