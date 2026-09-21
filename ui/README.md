# ui/

The page that `diffnote export` writes and `diffnote serve` serves.

## The exported page (`client/`)

A client-side app, drawn in the browser from data that Rust works out
(`src/html/viewmodel.rs`; the format is documented there) and embeds in the
page as JSON. Everything is in the one HTML file, as plain scripts, because it
is opened from a file:

- no `<script type="module" src>`, no `import()`, no `fetch`/XHR, no workers
  (a page opened from `file://` can't use them; a test in `src/html.rs` checks
  the scripts for them);
- what is kept in the browser (`localStorage`) or copied (`navigator.clipboard`)
  is wrapped so that it works without them.

Files, in the order they are put in the page (`CLIENT_LIBS`, `CLIENT_API`, `CLIENT_APP` in `src/html.rs`):

- `vendor/` — Preact, its hooks and htm (plain-script builds, no build step;
  see `vendor/README.md` for versions and licenses).
- `client/lib.js` — pure helpers (locations, which lines a thread covers, side
  by side pairing, preview text, ...). Tested with Node:
  `node --test ui/client/test/*.test.js`.
- `client/interact.js` — the mouse and keyboard on the document: a thread's
  range while hovered or pinned, jumping from the thread list, copy buttons.
  Done directly on the document (marking a range touches only its lines), not
  through components.
- `client/app.js` — the components and `Diffnote.start()`.
- `style.css` — the style (shared with the served page).

Placement of threads (re-anchoring), the order of the thread list, colors, and
comment HTML stay in Rust; the client only lays them out.

## The served page

`diffnote serve` serves the same app with one more script, `client/api.js`,
which talks to the server (`fetch`; only the served page has it, and a test
checks that the exported one doesn't). The model then has `interactive: true`
and `events` (how long the review's log was). The app changes the model in
its state: a change is shown at once and put right by the server's answer,
which says how many events the review has and how many the change added; if
they don't add up (the review changed under the page), or the window is looked
at again and the log has grown, the whole model is fetched again.

- Choosing lines (press, drag, Shift+click a line number) and the boxes for
  new threads: `useCompose` in `client/app.js`; the counters of the chosen
  lines are worked out by `lib.flatRows` / `lib.counters`.
- Other files: the tree and opened files are read as data
  (`/api/files/{rev}/tree|open|more`); opened files are held by the page
  (`useOpened`), not in the model.

Check syntax with `node --check` (CI does). The page is tested in a real
browser by `tests/browser` (see its README).
