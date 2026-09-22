// The bundle of the page `diffnote export` writes: one self-contained HTML
// file, opened from a file, that makes no requests. It is this entry, and not
// a switch inside the app, that leaves out `api.js` -- so nothing that could
// reach a server is in the bundle at all.
import { start } from './app.jsx';
import { interact } from './interact.js';
import { lib } from './lib.ts';

window.Diffnote = { lib, interact, start };
