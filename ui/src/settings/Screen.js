// The screen behind the title: one of the panes below, with the list of them.
import { useEffect } from 'preact/hooks';
import { lib } from '../lib.js';
import { html } from '../html.ts';
import { AttachmentsPane } from './Attachments.js';
import { SettingsFormPane } from './Form.js';
import { GeneralPane } from './General.js';
import { UserSettingsPane } from './User.js';

// The left-hand nav of the settings screen: which of its sections is shown.
var SETTINGS_SECTIONS = ['general', 'settings', 'attachments', 'user'];

function sectionLabel(key) {
  return lib.m('ui.settings.' + key + '_tab');
}

function SettingsNav(props) {
  return html`<nav class="diffnote-settings-nav" aria-label=${lib.m('ui.settings.nav_label')}>
    <ul>
      ${SETTINGS_SECTIONS.map(function (key) {
        return html`<li key=${key}><button type="button" class=${'diffnote-settings-nav__item' + (props.current === key ? ' is-current' : '')}
          aria-current=${props.current === key ? 'page' : undefined} data-diffnote-settings-nav=${key}
          onClick=${function () { props.onSelect(key); }}>${sectionLabel(key)}</button></li>`;
      })}
    </ul>
  </nav>`;
}

// The settings screen, in place of the review (the review is hidden, not
// taken down, while it is shown): a left-hand nav picks which of the
// sections above is shown on the right, GitHub-repo-settings style.
export function SettingsScreen(props) {
  useEffect(function () {
    var key = function (e) { if (e.key === 'Escape') props.onClose(); };
    document.addEventListener('keydown', key);
    return function () { document.removeEventListener('keydown', key); };
  }, []);
  return html`<main class="diffnote-settings" data-diffnote-settings-page data-diffnote-settings-section=${props.section}>
    <p class="diffnote-settings__top"><button type="button" class="diffnote-button" data-diffnote-settings-back onClick=${props.onClose}>${lib.m('ui.settings.back_button')}</button></p>
    <div class="diffnote-settings__layout">
      <${SettingsNav} current=${props.section} onSelect=${props.onSelect} />
      <div class="diffnote-settings__pane">
        ${props.section === 'general' && html`<${GeneralPane} model=${props.model} pending=${props.pending} note=${props.note} onPull=${props.onPull} />`}
        ${props.section === 'settings' && html`<${SettingsFormPane} model=${props.model} save=${props.saveSettings} />`}
        ${props.section === 'attachments' && html`<${AttachmentsPane} model=${props.model} remove=${props.removeAttached}
          onShow=${props.onShowThread} placementOf=${props.placementOf} />`}
        ${props.section === 'user' && html`<${UserSettingsPane} model=${props.model} save=${props.saveUserSettings} />`}
      </div>
    </div>
  </main>`;
}
