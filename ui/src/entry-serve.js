// The bundle of the page `diffnote serve` serves: the same app, plus the one
// thing an exported page doesn't have -- a server to talk to.
import { api } from './api.js';
import { start } from './app.js';
import { interact } from './interact.js';
import { lib } from './lib.js';
import { setTransport } from './transport.js';

setTransport(api);

window.Diffnote = { lib, interact, api, start };
