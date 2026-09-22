// Who the comments are written by, and the way to change it.
import { lib } from './lib.js';
import { html } from './html.ts';

// The name comments are written under, at the foot of the side, like the
// user who is signed in. Pressing it opens the user settings screen (like
// the title opens the review's settings).
export function UserChip(props) {
  var name = props.name;
  var initial = Array.from(name.trim())[0] || '?';
  return html`<div class="diffnote-user" data-diffnote-user>
    <span class="diffnote-user__avatar" aria-hidden="true">${initial.toUpperCase()}</span>
    <button type="button" class="diffnote-user__button" data-diffnote-user-settings title=${lib.m('ui.user.settings_title')} aria-haspopup="dialog"
      aria-pressed=${props.open} onClick=${props.onToggle}><strong data-diffnote-author>${name}</strong><span class="diffnote-user__icon" aria-hidden="true">⚙</span></button>
  </div>`;
}
