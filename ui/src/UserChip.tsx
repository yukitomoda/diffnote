// Who the comments are written by, and the way to change it.
import { lib } from './lib.ts';
import { Icon } from './icon.tsx';

// The name comments are written under, at the foot of the side, like the
// user who is signed in. Pressing it opens the user settings screen (like
// the title opens the review's settings).
interface UserChipProps {
  name: string;
  /** Whether its screen is the one open. */
  open: boolean;
  onToggle(): void;
}

export function UserChip(props: UserChipProps) {
  var name = props.name;
  var initial = Array.from(name.trim())[0] || '?';
  // The whole chip is the button, the picture of the name included: it is one
  // thing to press, and a small target beside an unpressable one is a miss
  // waiting to happen.
  return <div class="diffnote-user" data-diffnote-user>
    <button type="button" class="diffnote-user__button" data-diffnote-user-settings title={lib.m('ui.user.settings_title')} aria-haspopup="dialog"
      aria-pressed={props.open} onClick={props.onToggle}><span class="diffnote-user__avatar" aria-hidden="true">{initial.toUpperCase()}</span><strong data-diffnote-author>{name}</strong><Icon name="settings" class="diffnote-user__icon" /></button>
  </div>;
}
