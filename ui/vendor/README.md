# Vendored libraries

Plain-script builds (they set globals; no modules, so the exported page works
from `file://`), copied unchanged from npm (source-map comments removed):

| file | package | license |
|---|---|---|
| `preact.min.js` | preact 10.24.3 (`dist/preact.min.js`, sets `preact`) | MIT (`LICENSE-preact`) |
| `hooks.umd.js` | preact 10.24.3 (`hooks/dist/hooks.umd.js`, sets `preactHooks`) | MIT (`LICENSE-preact`) |
| `htm.js` | htm 3.1.1 (`dist/htm.js`, sets `htm`) | Apache-2.0 (`LICENSE-htm`) |

To update, download the same files from a CDN mirror of npm, e.g.
`https://cdn.jsdelivr.net/npm/preact@X/dist/preact.min.js`.
