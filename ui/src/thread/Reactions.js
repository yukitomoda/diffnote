// The emoji on a comment, and the table to pick one from.
import { useEffect, useRef, useState } from 'preact/hooks';
import { EMOJI } from '../emoji.js';
import { lib } from '../lib.js';
import { html } from '../html.js';

// The reactions to a comment: what people reacted with and how many (the
// ones of the name signed in are marked, and pressing one takes it back or
// adds one's own), and a button for another. A page that only shows the review
// has only the marks.
export function Reactions(props) {
  var c = props.comment;
  var actions = props.actions;
  var list = c.reactions || [];
  if (!actions && list.length === 0) return null;
  var me = actions ? actions.author : null;
  return html`<div class="diffnote-reactions" data-diffnote-reactions>
    ${list.map(function (r) {
      var mine = me != null && r.authors.indexOf(me) >= 0;
      var tip = lib.mf('ui.reaction.tip', { authors: r.authors.join(lib.m('ui.list_separator')), emoji: r.emoji });
      return actions
        ? html`<button type="button" key=${r.emoji} class=${'diffnote-reaction' + (mine ? ' is-mine' : '')} data-diffnote-reaction=${r.emoji}
            aria-pressed=${mine} title=${tip} onClick=${function () { actions.react(c.id, r.emoji); }}>${r.emoji}<span>${r.authors.length}</span></button>`
        : html`<span key=${r.emoji} class="diffnote-reaction" data-diffnote-reaction=${r.emoji} title=${tip}>${r.emoji}<span>${r.authors.length}</span></span>`;
    })}
    ${actions && html`<${EmojiButton} side="left" name="react" label="＋" title=${lib.m('ui.reaction.add_title')} buttonClass="diffnote-reaction diffnote-reaction--add"
      onPick=${function (emoji) { actions.react(c.id, emoji); }} />`}
  </div>`;
}

// The button that opens the table of emoji, and the table: a search box and the
// emoji that fit it; one chosen goes to `onPick`.
export function EmojiButton(props) {
  var _o = useState(false);
  var open = _o[0];
  var setOpen = _o[1];
  var _q = useState('');
  var query = _q[0];
  var setQuery = _q[1];
  var box = useRef(null);
  var input = useRef(null);
  useEffect(function () { if (open && input.current) input.current.focus(); }, [open]);
  useEffect(function () {
    if (!open) return undefined;
    var away = function (e) { if (box.current && !box.current.contains(e.target)) setOpen(false); };
    var key = function (e) { if (e.key === 'Escape') { e.stopPropagation(); setOpen(false); } };
    document.addEventListener('mousedown', away);
    document.addEventListener('keydown', key, true);
    return function () {
      document.removeEventListener('mousedown', away);
      document.removeEventListener('keydown', key, true);
    };
  }, [open]);
  var found = lib.findEmoji(EMOJI, query);
  var pick = function (ch) { setOpen(false); setQuery(''); props.onPick(ch); };
  return html`<span class=${'diffnote-emoji' + (props.side === 'left' ? ' diffnote-emoji--left' : '')} ref=${box}>
    <button type="button" class=${props.buttonClass || 'diffnote-attach diffnote-emoji__open'} data-diffnote-emoji-button=${props.name || ''} aria-haspopup="true" aria-expanded=${open}
      title=${props.title || lib.m('ui.emoji.default_title')} onClick=${function () { setOpen(!open); }}>${props.label || '😀'}</button>
    ${open && html`<div class="diffnote-emoji__panel" data-diffnote-emoji-panel>
      <input ref=${input} type="search" class="diffnote-emoji__search" data-diffnote-emoji-search placeholder=${lib.m('ui.emoji.search_placeholder')} value=${query}
        onInput=${function (e) { setQuery(e.target.value); }}
        onKeyDown=${function (e) { if (e.key === 'Enter') { e.preventDefault(); if (found[0]) pick(found[0][0]); } }} />
      <div class="diffnote-emoji__grid">
        ${found.map(function (e) {
          return html`<button type="button" key=${e[1]} class="diffnote-emoji__item" data-diffnote-emoji=${e[1]} title=${':' + e[1] + ': ' + e[2]}
            onClick=${function () { pick(e[0]); }}>${e[0]}</button>`;
        })}
      </div>
      ${found.length === 0 && html`<p class="diffnote-emoji__none">${lib.m('ui.emoji.none_found')}</p>`}
    </div>`}
  </span>`;
}
