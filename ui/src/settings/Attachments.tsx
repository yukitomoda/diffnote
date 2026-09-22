// 添付: what is attached, what uses it, and taking one out.
import { h } from 'preact';
import { useMemo, useState } from 'preact/hooks';
import { interact } from '../interact.ts';
import { lib } from '../lib.ts';
import type { AttachedData, Placement, ViewModel } from '../model.ts';
import type { ChangeAnswer } from '../state/contexts.ts';

// 添付: what the comments have attached, and what uses it. Unused ones are
// dropped at 終了 anyway; this is where to see them, save one, or take one
// out on the spot.
export interface AttachmentsProps {
  model: ViewModel;
  remove(attached: AttachedData): Promise<ChangeAnswer>;
  /** Going to the comment that shows one, and where that comment is. */
  onShow(thread: string): void;
  placementOf(thread: string): Placement | undefined;
}

export function AttachmentsPane(props: AttachmentsProps) {
  var model = props.model;
  var listed = (model.bundle && model.bundle.attachments) || [];
  var _e = useState('');
  var error = _e[0];
  var setError = _e[1];
  // The one being asked about before it goes, by id.
  var _a = useState<string | null>(null);
  var ask = _a[0];
  var setAsk = _a[1];
  var uses = useMemo(function () { return lib.attachmentUses(model.threads); }, [model.threads]);
  // Unused first (the ones worth clearing out), then as the server sorted
  // them: biggest first.
  var order = useMemo(function () {
    return listed.slice().sort(function (a, b) {
      return Number((uses[a.id] || []).length > 0) - Number((uses[b.id] || []).length > 0);
    });
  }, [listed, uses]);
  var total = listed.reduce(function (n, a) { return n + a.size; }, 0);
  var remove = function (a: AttachedData) {
    setAsk(null);
    setError('');
    props.remove(a).then(function (res) {
      if (!res.ok) setError(res.error || lib.m('ui.attachments.delete_failed'));
    });
  };
  var nameOf = function (a: AttachedData) {
    var named = (uses[a.id] || []).filter(function (u) { return u.name; })[0];
    return named ? named.name : lib.m('ui.attachments.no_name');
  };
  // What it is saved as. A file is called what the comment's link says (a
  // real file name); an image's text there is a description, not a name, so
  // it is saved by its digest, with the extension its type usually has.
  var fileName = function (a: AttachedData) {
    var named = a.kind === 'file' && (uses[a.id] || []).filter(function (u) { return u.name; })[0];
    if (named) return named.name;
    var ext = (a.media_type || '').split('/')[1];
    return 'diffnote-' + a.id.slice(0, 12) + (ext ? '.' + ext.replace('+xml', '') : '');
  };
  return <div data-diffnote-attachments-pane>
    <h2>{lib.m('ui.attachments.heading')}</h2>
    <p class="diffnote-settings__note">{lib.m('ui.attachments.note')}</p>
    {listed.length === 0
      ? <p class="diffnote-attached__empty">{lib.m('ui.attachments.empty')}</p>
      : <><p class="diffnote-attached__total">{lib.mf('ui.attachments.total', { count: String(listed.length), size: lib.formatSize(total) })}</p><ul class="diffnote-attached">
          {order.map(function (a) {
            var used = uses[a.id] || [];
            var image = a.kind === 'image';
            var href = (image ? '/api/images/' : '/api/attachments/') + a.id
              + (image ? '' : '?name=' + encodeURIComponent(fileName(a)));
            return <li key={a.id} class="diffnote-attached__item" data-diffnote-attached={a.id}>
              <div class="diffnote-attached__thumb">{image
                ? <button type="button" class="diffnote-attached__zoom" data-diffnote-attached-zoom={a.id}
                    title={lib.m('ui.attachments.zoom')} aria-label={lib.m('ui.attachments.zoom')}
                    onClick={function () { interact.zoom('/api/images/' + a.id, nameOf(a)); }}>
                    {h('img', { src: '/api/images/' + a.id, alt: '' })}
                  </button>
                : <span class="diffnote-attached__clip" aria-hidden="true">📎</span>}</div>
              <div class="diffnote-attached__what">
                <p class="diffnote-attached__name">{nameOf(a)}{used.length === 0 && <span class="diffnote-badge" data-diffnote-attached-unused>{lib.m('ui.attachments.unused')}</span>}</p>
                <p class="diffnote-attached__meta">{image ? lib.m('ui.attachments.image_kind') : lib.m('ui.attachments.file_kind')} ・ {a.media_type || ''}{a.media_type ? ' ・ ' : ''}{lib.formatSize(a.size)}</p>
                {used.length > 0 && <p class="diffnote-attached__uses" data-diffnote-attached-uses>
                  {lib.mf('ui.attachments.used_by', { n: String(used.length) })}{used.map(function (u, i) {
                    return <button key={i} type="button" class="diffnote-attached__use" data-diffnote-attached-use={u.thread}
                      onClick={function () { props.onShow(u.thread); }}>{lib.shortLocation(props.placementOf(u.thread))}</button>;
                  })}
                </p>}
              </div>
              <div class="diffnote-attached__buttons">
                <a class="diffnote-button" data-diffnote-attached-download={a.id} href={href} download={fileName(a)}>{lib.m('ui.attachments.download')}</a>
                <button type="button" class="diffnote-button" data-diffnote-attached-delete={a.id}
                  onClick={function () { setAsk(a.id); }}>{lib.m('ui.attachments.delete')}</button>
              </div>
              {ask === a.id && <div class="diffnote-attached__warn" role="alert" data-diffnote-attached-warn>
                <p>{used.length > 0
                  ? lib.mf('ui.attachments.confirm_used', { name: nameOf(a) })
                  : lib.mf('ui.attachments.confirm_unused', { name: nameOf(a) })}</p>
                <div class="diffnote-reply__buttons">
                  <button type="button" class="diffnote-button diffnote-button--danger" data-diffnote-attached-delete-ok
                    onClick={function () { remove(a); }}>{lib.m('ui.attachments.confirm_delete')}</button>
                  <button type="button" class="diffnote-button" onClick={function () { setAsk(null); }}>{lib.m('ui.confirm_cancel')}</button>
                </div>
              </div>}
            </li>;
          })}
        </ul></>}
    {error && <p class="diffnote-error" role="alert">{error}</p>}
  </div>;
}
