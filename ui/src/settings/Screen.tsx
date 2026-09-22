// The screen behind the title: one of the panes below, with the list of them.
import { useEffect } from 'preact/hooks';
import { lib } from '../lib.ts';
import { AttachmentsPane } from './Attachments.jsx';
import { SettingsFormPane } from './Form.jsx';
import { GeneralPane } from './General.jsx';
import { UserSettingsPane } from './User.jsx';
import type { ViewModel } from '../model.ts';
import type { GeneralProps } from './General.tsx';
import type { FormProps } from './Form.tsx';
import type { AttachmentsProps } from './Attachments.tsx';
import type { UserProps } from './User.tsx';

// The left-hand nav of the settings screen: which of its sections is shown.
export type Section = 'general' | 'settings' | 'attachments' | 'user';

var SETTINGS_SECTIONS: Section[] = ['general', 'settings', 'attachments', 'user'];

function sectionLabel(key: Section) {
  return lib.m('ui.settings.' + key + '_tab');
}

interface NavProps {
  current: Section;
  onSelect(section: Section): void;
}

function SettingsNav(props: NavProps) {
  return <nav class="diffnote-settings-nav" aria-label={lib.m('ui.settings.nav_label')}>
    <ul>
      {SETTINGS_SECTIONS.map(function (key) {
        return <li key={key}><button type="button" class={'diffnote-settings-nav__item' + (props.current === key ? ' is-current' : '')}
          aria-current={props.current === key ? 'page' : undefined} data-diffnote-settings-nav={key}
          onClick={function () { props.onSelect(key); }}>{sectionLabel(key)}</button></li>;
      })}
    </ul>
  </nav>;
}

// The settings screen, in place of the review (the review is hidden, not
// taken down, while it is shown): a left-hand nav picks which of the
// sections above is shown on the right, GitHub-repo-settings style.
interface ScreenProps extends Omit<GeneralProps, 'model'> {
  section: Section;
  model: ViewModel;
  onSelect(section: Section): void;
  onClose(): void;
  saveSettings: FormProps['save'];
  saveUserSettings: UserProps['save'];
  removeAttached: AttachmentsProps['remove'];
  onShowThread: AttachmentsProps['onShow'];
  placementOf: AttachmentsProps['placementOf'];
}

export function SettingsScreen(props: ScreenProps) {
  useEffect(function () {
    var key = function (e: KeyboardEvent) { if (e.key === 'Escape') props.onClose(); };
    document.addEventListener('keydown', key);
    return function () { document.removeEventListener('keydown', key); };
  }, []);
  return <main class="diffnote-settings" data-diffnote-settings-page data-diffnote-settings-section={props.section}>
    <p class="diffnote-settings__top"><button type="button" class="diffnote-button" data-diffnote-settings-back onClick={props.onClose}>{lib.m('ui.settings.back_button')}</button></p>
    <div class="diffnote-settings__layout">
      <SettingsNav current={props.section} onSelect={props.onSelect} />
      <div class="diffnote-settings__pane">
        {props.section === 'general' && <GeneralPane model={props.model} pending={props.pending} note={props.note} onPull={props.onPull} />}
        {props.section === 'settings' && <SettingsFormPane model={props.model} save={props.saveSettings} />}
        {props.section === 'attachments' && <AttachmentsPane model={props.model} remove={props.removeAttached}
          onShow={props.onShowThread} placementOf={props.placementOf} />}
        {props.section === 'user' && <UserSettingsPane model={props.model} save={props.saveUserSettings} />}
      </div>
    </div>
  </main>;
}
