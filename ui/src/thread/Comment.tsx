// One comment: its text, who wrote it, and what can be done to it.
import { useContext, useEffect, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { useAutoGrow } from '../dom.ts';
import { markdown } from '../markdown.tsx';
import { LinksContext } from '../state/contexts.ts';
import { Reactions } from './Reactions.jsx';
import { useAttach } from './attach.jsx';
import type { CommentData } from '../model.ts';
import type { Actions } from '../state/contexts.ts';
import { Icon } from '../icon.tsx';

function Time(props: { at: string }) {
  var t = new Date(props.at);
  return <time class="diffnote-comment__time" datetime={props.at} title={isNaN(t.getTime()) ? props.at : t.toLocaleString()}>{lib.formatTime(props.at)}</time>;
}

// One comment. One added since the server started has buttons to edit it and
// to take it out (the first comment of a thread takes the whole thread out).
/** Text chosen in a comment, and where to offer to quote it. */
interface Quote {
  text: string;
  top: number;
  left: number;
}

/** A change that is asked about first, and why it is. */
interface Ask {
  kind: 'edit' | 'delete';
  reasons: string[];
}

interface CommentProps {
  comment: CommentData;
  actions: Actions | null;
  threadId: string;
  /** Whether it is the thread's first comment, and how many replies stand on it. */
  first: boolean;
  replies: number;
}

export function Comment(props: CommentProps) {
  var c = props.comment;
  var actions = props.actions;
  var links = useContext(LinksContext);
  // (A comment that was deleted is only a mark that it was: nothing to change.)
  var canChange = !!actions && actions!.editable.has(c.id) && !c.deleted;
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
  // What is asked before a change goes ahead: `{ kind, reasons }`.
  var _a = useState<Ask | null>(null);
  var ask = _a[0];
  var setAsk = _a[1];
  var attach = useAttach(text, setText);
  var field = useRef(null);
  useAutoGrow(field, text, editing);
  // Text chosen in this comment's body, with where to offer to quote it.
  var body = useRef<HTMLDivElement | null>(null);
  var _q = useState<Quote | null>(null);
  var quote = _q[0];
  var setQuote = _q[1];
  var look = function () {
    var sel = window.getSelection && window.getSelection();
    var el = body.current;
    if (!sel || sel.isCollapsed || sel.rangeCount === 0 || !el || !el.contains(sel.anchorNode) || !el.contains(sel.focusNode)) { setQuote(null); return; }
    var chosen = sel.toString();
    if (chosen.trim() === '') { setQuote(null); return; }
    var rect = sel.getRangeAt(0).getBoundingClientRect();
    setQuote({ text: chosen, top: Math.max(rect.top - 34, 4), left: Math.min(Math.max(rect.left, 4), window.innerWidth - 150) });
  };
  useEffect(function () {
    if (!quote) return undefined;
    var away = function () { setQuote(null); };
    var changed = function () {
      var sel = window.getSelection();
      if (!sel || sel.isCollapsed) setQuote(null);
    };
    document.addEventListener('selectionchange', changed);
    window.addEventListener('scroll', away, true);
    return function () {
      document.removeEventListener('selectionchange', changed);
      window.removeEventListener('scroll', away, true);
    };
  }, [!!quote]);
  // Quoting: what was chosen, or (from the menu) the whole comment as it was written.
  var quoteIt = function (text: string) {
    document.dispatchEvent(new CustomEvent('diffnote:quote', { detail: { thread: props.threadId, text: text } }));
    var sel = window.getSelection && window.getSelection();
    if (sel) sel.removeAllRanges();
    setQuote(null);
  };
  var quoteWhole = function () {
    var sel = window.getSelection && window.getSelection();
    var el = body.current;
    var chosen = sel && !sel.isCollapsed && el && el.contains(sel.anchorNode) ? sel.toString() : '';
    quoteIt(chosen.trim() !== '' ? chosen : c.body || lib.plainText(c.doc));
  };
  var save = function () {
    if (!text.trim() || busy) return;
    setBusy(true);
    setError('');
    actions!.edit(c.id, text).then(function (res) {
      setBusy(false);
      if (res.ok) setEditing(false);
      else setError(res.error || lib.m('ui.save_failed'));
    });
  };
  var doRemove = function () {
    setAsk(null);
    setBusy(true);
    setError('');
    actions!.remove(c.id).then(function (res) {
      setBusy(false);
      if (!res.ok) setError(res.error || lib.m('ui.comment.delete_failed'));
    });
  };
  var startEdit = function () { setText(c.body || ''); setError(''); setAsk(null); setEditing(true); };
  // A comment somebody else wrote is asked about first (whoever is signed in as
  // another name), so that it isn't changed by mistake; so is any delete.
  var others = actions && actions!.author != null && c.author !== actions!.author;
  var whose = others ? lib.mf('ui.comment.whose', { author: c.author, me: actions!.author || '' }) : '';
  var change = function (kind: Ask['kind']) {
    var reasons: string[] = [];
    if (others) reasons.push(whose);
    if (kind === 'delete') {
      if (props.first && props.replies > 0) {
        reasons.push(lib.mf('ui.comment.delete_reason_replies', { n: String(props.replies) }));
      } else if (props.first) {
        reasons.push(lib.m('ui.comment.delete_reason_thread'));
      } else {
        reasons.push(lib.m('ui.comment.delete_reason_reply'));
      }
    }
    if (reasons.length === 0) { startEdit(); return; }
    setAsk({ kind: kind, reasons: reasons });
  };
  return <article class="diffnote-comment" data-diffnote-comment={c.id} data-diffnote-mine={actions && !others ? '' : undefined} data-diffnote-changed={actions && actions!.changed && actions!.changed.has(c.id) && !c.deleted ? '' : undefined}>
    <p class="diffnote-comment__author">{c.author}<Time at={c.at} />{actions && !c.deleted && !editing && <CommentMenu busy={busy} canChange={canChange}
      onQuote={quoteWhole} onEdit={function () { change('edit'); }} onDelete={function () { change('delete'); }} />}</p>
    {ask && <div class="diffnote-comment__warn" role="alert" data-diffnote-warn>
      {ask!.reasons.map(function (r: string, i: number) { return <p key={i}>{r}</p>; })}
      <div class="diffnote-reply__buttons">
        <button type="button" class={'diffnote-button ' + (ask!.kind === 'delete' ? 'diffnote-button--danger' : 'diffnote-button--primary')} data-diffnote-warn-ok
          onClick={function () { if (ask!.kind === 'delete') doRemove(); else startEdit(); }}>{ask!.kind === 'delete' ? lib.m('ui.comment.delete_button') : lib.m('ui.comment.edit_button')}</button>
        <button type="button" class="diffnote-button" data-diffnote-warn-cancel onClick={function () { setAsk(null); }}>{lib.m('ui.confirm_cancel')}</button>
      </div>
    </div>}
    {editing
      ? <form class="diffnote-compose" data-diffnote-edit-form onSubmit={function (e) { e.preventDefault(); save(); }}>
          <div class="diffnote-attach-bar">{attach.picker(function () { return field.current; })}</div>
          <textarea ref={field} rows={3} value={text} {...attach.handlers} onInput={function (e) { setText(e.currentTarget.value); }}
            onKeyDown={function (e) {
              if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); save(); }
              if (e.key === 'Escape') { e.stopPropagation(); setEditing(false); }
            }}></textarea>
          {attach.note}
          <div class="diffnote-reply__buttons">
            <button type="submit" class="diffnote-button diffnote-button--primary" disabled={busy}>{lib.m('ui.save_button')}</button>
            <button type="button" class="diffnote-button" onClick={function () { setEditing(false); }}>{lib.m('ui.cancel_button')}</button>
          </div>
          {error && <p class="diffnote-error">{error}</p>}
        </form>
      : c.deleted
        ? <p class="diffnote-comment__deleted" data-diffnote-deleted>{lib.m('ui.comment.deleted_notice')}</p>
        : <><div class="diffnote-comment__body" ref={body} onMouseUp={function () { setTimeout(look, 0); }} onKeyUp={look}>{markdown(c.doc, links)}</div>{error && <p class="diffnote-error">{error}</p>}<Reactions comment={c} actions={actions} /></>}
    {quote && <button type="button" class="diffnote-quote-button" data-diffnote-quote-selection style={'top:' + quote.top + 'px;left:' + quote.left + 'px'}
      onMouseDown={function (e) { e.preventDefault(); }} onClick={function () { quoteIt(quote!.text); }}><Icon name="quote" />{' '}{lib.m('ui.comment.quote_button')}</button>}
  </article>;
}

// The menu of a comment: what can be done to it (in the page while it is
// shut, only not shown).
interface MenuProps {
  busy?: boolean;
  canChange: boolean;
  onQuote(): void;
  onEdit(): void;
  onDelete(): void;
}

function CommentMenu(props: MenuProps) {
  var _o = useState(false);
  var open = _o[0];
  var setOpen = _o[1];
  var box = useRef<HTMLSpanElement | null>(null);
  useEffect(function () {
    if (!open) return undefined;
    var away = function (e: MouseEvent) { if (box.current && !box.current.contains(e.target as Node)) setOpen(false); };
    var key = function (e: KeyboardEvent) { if (e.key === 'Escape') setOpen(false); };
    document.addEventListener('mousedown', away);
    document.addEventListener('keydown', key);
    return function () {
      document.removeEventListener('mousedown', away);
      document.removeEventListener('keydown', key);
    };
  }, [open]);
  return <span class="diffnote-comment__menu" ref={box}>
    <button type="button" class="diffnote-comment__more" data-diffnote-comment-menu aria-label={lib.m('ui.comment.menu_label')} aria-haspopup="true" aria-expanded={open}
      onClick={function () { setOpen(!open); }}><Icon name="menu" /></button>
    <span class="diffnote-comment__panel" hidden={!open}>
      <button type="button" class="diffnote-comment__item" data-diffnote-quote
        onClick={function () { setOpen(false); props.onQuote(); }}>{lib.m('ui.comment.menu_quote')}</button>
      {props.canChange && <><button type="button" class="diffnote-comment__item" data-diffnote-edit disabled={props.busy}
        onClick={function () { setOpen(false); props.onEdit(); }}>{lib.m('ui.comment.menu_edit')}</button><button type="button" class="diffnote-comment__item diffnote-comment__item--danger" data-diffnote-delete disabled={props.busy}
        onClick={function () { setOpen(false); props.onDelete(); }}>{lib.m('ui.comment.menu_delete')}</button></>}
    </span>
  </span>;
}
