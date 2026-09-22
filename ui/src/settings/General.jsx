// 全般: what the review is, and what can be done with the whole of it.
import { lib } from '../lib.ts';

// 全般: what pressing 最新を取り込む/ダウンロード/エクスポート did before this
// was a screen of its own -- bundle-wide operations, not something kept.
export function GeneralPane(props) {
  var model = props.model;
  var bundle = model.bundle;
  var row = function (label, value) { return <div><dt>{label}</dt><dd>{value}</dd></div>; };
  return <div data-diffnote-general-pane>
    <h2>{lib.m('ui.settings.general_heading')}</h2>
    {model.refreshable && <div class="diffnote-settings__action">
      <button type="button" class={'diffnote-button' + (props.pending ? ' diffnote-button--primary' : '')} data-diffnote-pull
        disabled={!!(props.note && props.note.busy)} onClick={props.onPull}>{lib.m('ui.settings.pull_button')}</button>
      <p class="diffnote-settings__action-note">{lib.m('ui.settings.pull_note')}</p>
      {props.note && <p class={'diffnote-pull__note' + (props.note.failed ? ' is-failed' : '')} data-diffnote-pull-note role="status">{props.note.text}</p>}
    </div>}
    <div class="diffnote-settings__action">
      <a class="diffnote-button" data-diffnote-download href="/download">{lib.m('ui.settings.download_button')}</a>
      <p class="diffnote-settings__action-note">{lib.m('ui.settings.download_note')}</p>
    </div>
    <div class="diffnote-settings__action">
      <a class="diffnote-button" data-diffnote-export href="/export">{lib.m('ui.settings.export_button')}</a>
      <p class="diffnote-settings__action-note">{lib.m('ui.settings.export_note')}</p>
    </div>
    {bundle && <dl class="diffnote-settings__info" data-diffnote-bundle-info>
      <h3>{lib.m('ui.settings.bundle_info_heading')}</h3>
      {row(lib.m('ui.settings.bundle_size_label'), lib.formatSize(bundle.size))}
      {row(lib.m('ui.settings.bundle_revisions_label'), lib.mf('ui.settings.bundle_revisions_value', { n: String(bundle.revisions) }))}
      {row(lib.m('ui.settings.bundle_images_label'), lib.mf('ui.settings.bundle_count_with_size', { count: String(bundle.images.count), size: lib.formatSize(bundle.images.bytes) }))}
      {row(lib.m('ui.settings.bundle_files_label'), lib.mf('ui.settings.bundle_count_with_size', { count: String(bundle.files.count), size: lib.formatSize(bundle.files.bytes) }))}
    </dl>}
  </div>;
}
