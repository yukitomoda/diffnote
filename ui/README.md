# ui/

The page that `diffnote export` writes and `diffnote serve` serves: its
style and script, embedded into the binary with `include_str!` (see
`src/html.rs`). They are ordinary files so an editor can check them; they are
not built or bundled.

- `style.css` — the page's style.
- `app.js` — the page's script (revision switching, thread highlighting, the
  hide-resolved box, and, on the served page, replying, resolving, choosing
  lines and commenting, other files).

Check the syntax with `node --check ui/app.js` (CI does).

Plan: the page is being moved to a client-side app (Preact + htm, no build
step) fed with data from Rust, so that layouts (unified/split) and future UI
changes live in one place. Placement of threads (re-anchoring) stays in Rust.
