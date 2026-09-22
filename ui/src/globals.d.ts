// What a bundle leaves on `window`: the page's own script calls
// `Diffnote.start()`, and the browser tests reach the rest of it.
import type { lib } from './lib.ts';
import type { Transport } from './transport.ts';

declare global {
  interface Window {
    Diffnote: {
      lib: typeof lib;
      interact: unknown;
      /** Only the served page's bundle has this. */
      api?: Transport;
      start: () => void;
    };
  }
}

export {};
