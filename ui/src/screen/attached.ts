// What the 添付 screen says of each attachment, apart from how it is drawn
// (Attachments.tsx), so that it can be tested without a page.
import type { AttachmentUse } from '../lib.ts';
import type { AttachedData } from '../model.ts';

export type Uses = Record<string, AttachmentUse[]>;

/** Unused first (the ones worth clearing out), then as the server sorted
 * them: biggest first. */
export function inOrder(listed: AttachedData[], uses: Uses): AttachedData[] {
  return listed.slice().sort(function (a, b) {
    return Number((uses[a.id] || []).length > 0) - Number((uses[b.id] || []).length > 0);
  });
}

/** What it goes by: the name of the file it was attached from, where the
 * review kept one, and otherwise what a comment calls it (for an image that
 * is its description, which is better than nothing); `null` if nothing does. */
export function nameOf(a: AttachedData, uses: Uses): string | null {
  if (a.name) return a.name;
  var named = (uses[a.id] || []).filter(function (u) { return u.name; })[0];
  return named ? named.name : null;
}

/** What it is saved as: its own name, then -- for a file -- what the
 * comment's link calls it. An image's text there is a description, not a
 * name, so one that came without a name is saved by its digest, with the
 * extension its type usually has. */
export function fileNameOf(a: AttachedData, uses: Uses): string {
  if (a.name) return a.name;
  var named = a.kind === 'file' && (uses[a.id] || []).filter(function (u) { return u.name; })[0];
  if (named) return named.name;
  var ext = (a.media_type || '').split('/')[1];
  return 'diffnote-' + a.id.slice(0, 12) + (ext ? '.' + ext.replace('+xml', '') : '');
}
