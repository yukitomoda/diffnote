// How the page reaches the server, or `null` when there is none.
//
// Only the served page has one: `entry-serve.ts` puts `api.ts` here, and
// `entry-export.ts` imports neither, so an exported page's bundle holds none of
// the code that would make a request (it is opened from a file). The app asks
// for `transport` and, where it may be `null`, treats that as "nothing to talk
// to".
import type { Answer, UploadAnswer } from './model.ts';

export interface Transport {
  /** A change: JSON in, JSON out. */
  post<T = object>(path: string, data?: unknown): Promise<Answer<T>>;
  get<T = object>(path: string): Promise<Answer<T>>;
  /** A picture: its bytes, and the name of the file it came from (a picture
   * pasted out of the clipboard came from none). */
  upload(blob: Blob, name?: string): Promise<UploadAnswer>;
  /** Any other file: its bytes, and what it is called. */
  uploadFile(blob: Blob, name: string): Promise<UploadAnswer>;
}

export let transport: Transport | null = null;

export function setTransport(api: Transport): void {
  transport = api;
}

/**
 * The server, where the page has one. Only asked for down a path that has
 * already found that the page is the served one (`model.interactive`).
 */
export function server(): Transport {
  if (!transport) throw new Error('diffnote: this page has no server');
  return transport;
}
