# ui/

The page that `diffnote export` writes and `diffnote serve` serves.

## Building it

The page is built with node, the binary with cargo, and the binary carries the
page (`src/html.rs` embeds `dist/` with `include_str!`). So:

```sh
mise run build     # the page, then the binary -- whichever half you changed
mise run test      # the same, then every test
```

`ui/dist` is built, not committed. Nothing is run by cargo: `build.rs` only
looks at whether `dist/` is there and was built from the `src/` on disk (it
compares the sum `build.mjs` writes into `dist/.sources`), and says to run
`mise run build` if not. `DIFFNOTE_SKIP_UI_CHECK=1` turns that off.

`esbuild` bundles `src/` into two plain scripts. They are minified, but only the
spacing and the comments: the names are left alone, so the page can still be
read in the browser's own tools, and there are no source maps to ship.

## The two bundles

Everything is in the one HTML file, as a plain script, because the page is
opened from a file:

- no `<script type="module" src>`, no `import()`, no `fetch`/XHR, no workers
  (a page opened from `file://` can't use them);
- what is kept in the browser (`localStorage`) or copied
  (`navigator.clipboard`) is wrapped so that it works without them.

Which is why there are two entries, and not one bundle with a switch in it:

| entry | bundle | for |
|---|---|---|
| `src/entry-export.js` | `dist/export.js` | the exported page: it imports no `api.js`, so none of the code that would make a request is in it at all |
| `src/entry-serve.js` | `dist/serve.js` | the served page: it puts `api.js` in `transport.js`, which the app asks for |

Each sets `window.Diffnote`, and the page calls `Diffnote.start()`.

What the page may and may not contain is checked in two places: the rules about
what we write are in `src/test/sources.test.js` (no `innerHTML`, no element
written as markup, only `api.js` talks to the server), and the ones about the
whole bundle, libraries and all, are in `src/html.rs`.

## The files

- `src/lib.js` — pure helpers (locations, which lines a thread covers, side by
  side pairing, preview text, ...). Tested with Node: `npm --prefix ui test`.
- `src/interact.js` — the mouse and keyboard on the document: a thread's range
  while hovered or pinned, jumping from the thread list, copy buttons, the
  zoomed picture. Done directly on the document (marking a range touches only
  its lines), not through components.
- `src/app.js` — the components and `start()`.
- `src/api.js` — talking to the server; only the served bundle has it.
- `src/emoji.js` — the emoji the page offers.
- `src/style.css` — the style (shared by both pages).

The data the page is drawn from is worked out by Rust (`src/html/viewmodel.rs`;
the format is documented there) and embedded in the page as JSON. Placement of
threads (re-anchoring), the order of the thread list, colors, and comment HTML
stay in Rust; the client only lays them out.

The libraries (preact, its hooks, htm) come from npm and are bundled in; see
`THIRD-PARTY.md`, since every binary and every exported page carries them.

## The served page

`diffnote serve` serves the same app with a server behind it. The model then has
`interactive: true` and `events` (how long the review's log was). The app changes
the model in its state: a change is shown at once and put right by the server's
answer, which says how many events the review has and how many the change added;
if they don't add up (the review changed under the page), or the window is looked
at again and the log has grown, the whole model is fetched again.

- Choosing lines (press, drag, Shift+click a line number) and the boxes for
  new threads: `useCompose` in `src/app.js`; the counters of the chosen lines
  are worked out by `lib.flatRows` / `lib.counters`.
- Other files: the tree and opened files are read as data
  (`/api/files/{rev}/tree|open|more`); opened files are held by the page
  (`useOpened`), not in the model.

The page is tested in a real browser by `tests/browser` (see its README).
