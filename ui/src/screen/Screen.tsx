// The screen behind the title: one of the panes below, with the list of them.
import { useEffect } from 'preact/hooks';
import { lib } from '../lib.ts';
import { SECTION_GROUPS } from '../state/route.ts';
import type { Section } from '../state/route.ts';
import { AttachmentsPane } from './Attachments.jsx';
import { TimelinePane } from './Timeline.jsx';
import { SettingsFormPane } from './Form.jsx';
import { GeneralPane } from './General.jsx';
import { UserSettingsPane } from './User.jsx';
import type { ViewModel } from '../model.ts';
import type { GeneralProps } from './General.tsx';
import type { FormProps } from './Form.tsx';
import type { AttachmentsProps } from './Attachments.tsx';
import type { UserProps } from './User.tsx';
import { Icon } from '../icon.tsx';

// Panes that are a list rather than a column of text.
const WIDE: Section[] = ['timeline', 'attachments'];

function sectionLabel(key: Section) {
  return lib.m('ui.screen.' + key + '_tab');
}

interface NavProps {
  current: Section;
  onSelect(section: Section): void;
}

function ScreenNav(props: NavProps) {
  return <nav class="diffnote-screen-nav" aria-label={lib.m('ui.screen.nav_label')}>
    {SECTION_GROUPS.map(function (group, g) {
      return <ul key={g}>
        {group.map(function (key) {
          return <li key={key}><button type="button" class={'diffnote-screen-nav__item' + (props.current === key ? ' is-current' : '')}
            aria-current={props.current === key ? 'page' : undefined} data-diffnote-screen-nav={key}
            onClick={function () { props.onSelect(key); }}>{sectionLabel(key)}</button></li>;
        })}
      </ul>;
    })}
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

export function ReviewScreen(props: ScreenProps) {
  useEffect(function () {
    var key = function (e: KeyboardEvent) { if (e.key === 'Escape') props.onClose(); };
    document.addEventListener('keydown', key);
    return function () { document.removeEventListener('keydown', key); };
  }, []);
  return <main class="diffnote-screen" data-diffnote-screen data-diffnote-screen-section={props.section}>
    <p class="diffnote-screen__top"><button type="button" class="diffnote-button" data-diffnote-screen-back onClick={props.onClose}><Icon name="back" />{' '}{lib.m('ui.screen.back_button')}</button></p>
    <div class="diffnote-screen__layout">
      <ScreenNav current={props.section} onSelect={props.onSelect} />
      <div class={'diffnote-screen__pane' + (WIDE.indexOf(props.section) >= 0 ? ' diffnote-screen__pane--wide' : '')}>
        {props.section === 'timeline' && <TimelinePane model={props.model}
          onShow={props.onShowThread} placementOf={props.placementOf} />}
        {props.section === 'general' && <GeneralPane model={props.model} pending={props.pending} note={props.note} onPull={props.onPull} />}
        {props.section === 'settings' && <SettingsFormPane model={props.model} save={props.saveSettings} />}
        {props.section === 'attachments' && <AttachmentsPane model={props.model} remove={props.removeAttached}
          onShow={props.onShowThread} placementOf={props.placementOf} />}
        {props.section === 'user' && <UserSettingsPane model={props.model} save={props.saveUserSettings} />}
      </div>
    </div>
  </main>;
}
