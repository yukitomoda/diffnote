// A thread's card: its comments, and the box to reply in.
import { useContext, useEffect, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { useAutoGrow } from '../dom.ts';
import { ActionsContext } from '../state/contexts.js';
import { Comment } from './Comment.jsx';
import { useAttach } from './attach.jsx';

// One thread as a card.
export function Card(props) {
  var t = props.thread;
  var p = props.placement;
  var actions = useContext(ActionsContext);
  var loc = lib.location(p);
  var color = p && p.kind === 'line' ? lib.color(p.color) : null;
  var absent = p && p.kind === 'point' ? p : null;
  return <details
    class={'diffnote-thread' + (t.resolved ? ' diffnote-thread--resolved' : '')}
    id={'r' + props.rev + '-thread-' + t.id}
    data-diffnote-thread-id={t.id}
    data-diffnote-color={color || '#57606a'}
    open={!t.resolved}
  >
    <summary>
      {color && <span class="diffnote-thread__swatch" style={'background:' + color}></span>}{t.resolved ? lib.m('ui.thread.resolved') : lib.m('ui.thread.unresolved')}{loc && <>{' '}<span class="diffnote-thread__where">{loc}</span></>}{absent && lib.m('ui.absence.' + absent.absence)}{loc &&
      <button type="button" class="diffnote-copy" data-diffnote-copy={loc + '@' + (props.rev + 1)} title={lib.m('ui.copy.location_title')}>{lib.m('ui.copy_button')}</button>}
    </summary>
    {absent && absent.was.length > 0 && <pre class="diffnote-deleted__snippet">{absent.was.join('\n') + '\n'}</pre>}
    {t.comments.map(function (c, i) {
      return <Comment key={c.id} comment={c} actions={actions} threadId={t.id} first={i === 0} replies={t.comments.length - 1}
        othersReplies={actions ? t.comments.slice(1).filter(function (x) { return x.author !== actions.author; }).length : 0} />;
    })}
    {actions && <Actions thread={t} actions={actions} />}
  </details>;
}

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
  useAutoGrow(field, text);
  // A quotation asked for (from a comment of this thread) goes in the box.
  useEffect(function () {
    var on = function (e) {
      if (!e.detail || e.detail.thread !== t.id) return;
      var box = field.current;
      if (!box) return;
      var details = box.closest('details');
      if (details) details.open = true;
      setText(function (cur) { return lib.appendQuote(cur, e.detail.text); });
      setTimeout(function () {
        box.focus();
        box.setSelectionRange(box.value.length, box.value.length);
        box.scrollIntoView({ block: 'nearest' });
      }, 0);
    };
    document.addEventListener('diffnote:quote', on);
    return function () { document.removeEventListener('diffnote:quote', on); };
  }, [t.id]);

  function send() {
    var body = text.trim();
    if (!body || pending !== null) return;
    setPending(body);
    setError(null);
    actions.reply(t.id, body).then(function (res) {
      setPending(null);
      if (res.ok) setText('');
      else setError(res.error || lib.m('ui.save_failed'));
    });
  }
  function toggle() {
    setError(null);
    actions.setResolved(t.id, !t.resolved).then(function (res) {
      if (!res.ok) setError(res.error || lib.m('ui.save_failed'));
    });
  }
  var action = t.resolved ? 'reopen' : 'resolve';
  return <>{pending !== null && <article class="diffnote-comment is-pending">
      <p class="diffnote-comment__author">{lib.m('ui.comment.saving')}</p>
      <div class="diffnote-comment__body">{pending}</div>
    </article>}<div class="diffnote-thread__actions">
      <form class="diffnote-reply" data-diffnote-thread={t.id} onSubmit={function (e) { e.preventDefault(); send(); }}>
        <div class="diffnote-attach-bar">{attach.picker(function () { return field.current; })}</div>
        <textarea ref={field} rows="2" placeholder={lib.m('ui.comment.reply_placeholder')} value={text} disabled={pending !== null}
          {...attach.handlers}
          onInput={function (e) { setText(e.target.value); }}
          onKeyDown={function (e) { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send(); } }}></textarea>
        {attach.note}
        <div class="diffnote-reply__buttons">
          <button type="submit" class="diffnote-button diffnote-button--primary">{lib.m('ui.comment.reply_button')}</button>
          <button type="button" class="diffnote-button" data-diffnote-action={action} data-diffnote-thread={t.id} onClick={toggle}>{t.resolved ? lib.m('ui.thread.reopen_button') : lib.m('ui.thread.resolve_button')}</button>
        </div>
        {error && <p class="diffnote-error">{error}</p>}
      </form>
    </div></>;
}
