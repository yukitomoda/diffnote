// The box a new thread is written in.
import { useContext, useEffect, useRef } from 'preact/hooks';
import { lib } from '../lib.js';
import { useAutoGrow } from '../dom.js';
import { html } from '../html.js';
import { ComposeContext } from '../state/contexts.js';
import { useAttach } from './attach.js';

// The box a new thread is written in: on chosen lines, on a file, or on the
// whole review. What is written is kept while the choice changes.
export function Composer(props) {
  var c = useContext(ComposeContext);
  var box = useRef(null);
  useEffect(function () { box.current.focus(); }, []);
  var send = function () { c.send(props.request); };
  var attach = useAttach(c.draft, c.setDraft);
  useAutoGrow(box, c.draft, c.pending);
  return html`<div>
    <form class="diffnote-compose" data-diffnote-scope=${props.scope} style=${c.pending ? 'display:none' : undefined}
      onSubmit=${function (e) { e.preventDefault(); send(); }}>
      <div class="diffnote-compose__head"><div class="diffnote-compose__where">${props.where}</div>${props.copy && html`<button type="button" class="diffnote-copy" data-diffnote-copy=${props.copy} title=${lib.m('ui.copy.range_title')}>${lib.m('ui.copy_button')}</button>`}<span class="diffnote-attach-bar">${attach.picker(function () { return box.current; })}</span></div>
      <textarea ref=${box} rows="3" placeholder=${lib.m('ui.compose.placeholder')} value=${c.draft} ...${attach.handlers}
        onInput=${function (e) { c.setDraft(e.target.value); }}
        onKeyDown=${function (e) { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send(); } }}></textarea>
      ${attach.note}
      <div class="diffnote-reply__buttons">
        <button type="submit" class="diffnote-button diffnote-button--primary">${lib.m('ui.compose.submit')}</button>
        <button type="button" class="diffnote-button" data-diffnote-cancel onClick=${c.close}>${lib.m('ui.cancel_button')}</button>
      </div>
      ${c.error && html`<p class="diffnote-error">${c.error}</p>`}
    </form>
    ${c.pending && html`<article class="diffnote-comment is-pending">
      <p class="diffnote-comment__author">${lib.m('ui.comment.saving')}</p>
      <div class="diffnote-comment__body">${c.draft}</div>
    </article>`}
  </div>`;
}
