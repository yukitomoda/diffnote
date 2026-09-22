// Talking to the server that serves the page. Only the served page has this
// file: an exported page makes no requests (it is opened from a file).
//
// Every call gives back an answer with `ok`; a failure to reach the server is
// `{ ok: false, error }`, like the server's own refusals.
import { lib } from './lib.ts';
import type { Answer, Refusal, UploadAnswer } from './model.ts';
import type { Transport } from './transport.ts';

// Not constants: the message table (see lib.ts) is only loaded once `start()`
// runs, after this file does.
const unreachable = (): Refusal => ({ ok: false, error: lib.m('ui.api.unreachable') });
const unreadable = (): Refusal => ({ ok: false, error: lib.m('ui.api.unreadable') });

/** The header is what makes the server accept a change (a page from another
 * site can't send it). */
const OURS = { 'X-Diffnote': '1' };

async function call<T>(path: string, init?: RequestInit): Promise<Answer<T>> {
  let response: Response;
  try {
    response = await fetch(path, { credentials: 'same-origin', ...init });
  } catch {
    return unreachable();
  }
  try {
    return (await response.json()) as Answer<T>;
  } catch {
    return unreadable();
  }
}

export const api: Transport = {
  post<T>(path: string, data?: unknown): Promise<Answer<T>> {
    return call<T>(path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...OURS },
      body: JSON.stringify(data || {}),
    });
  },

  get<T>(path: string): Promise<Answer<T>> {
    return call<T>(path);
  },

  upload(blob: Blob, name?: string): Promise<UploadAnswer> {
    return call((name ? '/api/images?name=' + encodeURIComponent(name) : '/api/images'), {
      method: 'POST',
      headers: { 'Content-Type': blob.type || 'application/octet-stream', ...OURS },
      body: blob,
    });
  },

  uploadFile(blob: Blob, name: string): Promise<UploadAnswer> {
    return call('/api/attachments?name=' + encodeURIComponent(name), {
      method: 'POST',
      headers: { 'Content-Type': 'application/octet-stream', ...OURS },
      body: blob,
    });
  },
};
