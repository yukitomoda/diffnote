// The bundle of the page `diffnote review` and `diffnote open` serve: the same app, plus the one
// thing an exported page doesn't have -- a server to talk to.
import { api } from './api.ts';
import { start } from './app.tsx';
import { interact } from './interact.ts';
import { lib } from './lib.ts';
import { setTransport } from './transport.ts';

setTransport(api);

window.Diffnote = { lib, interact, api, start };
