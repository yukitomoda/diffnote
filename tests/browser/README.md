# Browser tests

End-to-end tests of the page (`diffnote export`, `diffnote review`) in a real
headless Chrome, driven over the DevTools protocol with a small client written
for the purpose (`harness.py`, standard library only). They use real mouse
and keyboard events, so what a person does (pressing a line number, dragging,
Shift+click, Escape) is what is tested.

The reviews they open are made by the real `diffnote`: `review` records the
revisions and its own API writes the comments, the way a person makes one.
Nothing is checked in as a fixture.

    mise run build
    python3 -m unittest discover -s tests/browser -v

Needs Python 3 (`mise.toml` pins it; the standard library is all it uses),
Chrome or Chromium (`CHROME=/path/to/chrome` to name it) and
`unzip`, and a built binary (`DIFFNOTE_BIN=...`, default `target/debug/diffnote`)
-- built after the page it carries, which is what `mise run build` does (see
ui/README.md).
A test module is skipped, not failed, when the browser or the binary is missing.

Waiting, which is what makes a test pass on one machine and fail on another:

- Never for a fixed time (`test_rules.py` refuses `sleep(` in a test).
  Wait for what has to be so: `b.wait(expr)` for something on the page,
  `b.settle()` for "the page has done with what it was doing" (no request
  to the server under way, and a few frames drawn since -- an effect, such
  as the one that adds a listener for Escape, runs a frame after it is
  drawn), `harness.until(check)` for something outside the page (a file,
  what the server printed).
- Load a page again with `b.reload()` / `b.open()`, never the page's own
  `location.reload()`: they wait for the new page, not for the old one that
  is still there for a moment (`test_rules.py` refuses that too).
- Choose lines with `b.choose(sel)` / `b.choose_lines(a, b)`: the events go
  to the elements, not to a point on the screen, which a page that moves in
  between puts another line under. `click_at`, `drag` and `hover` (a real
  pointer at a measured point) are for the few tests about the pointer
  itself: dragging, Shift+click, hovering a range.
- What can be checked without a page (a function of `ui/src/*.ts`, what the
  server answers) is checked in node or Rust, not here.
- `DIFFNOTE_CPU_SLOWDOWN=4` runs every page as on a machine four times
  slower. A new test should pass so as well: a CI runner is slower than the
  machine it was written on.

Tips for writing more:

- `Browser.wait(expr)` needs a value that comes back by value: wrap a DOM node
  in `!!`.
- A click applies its change on the page at once (a reply as a faded comment,
  a resolve as the new state), and the server's answer swaps the real thing in:
  wait for what the *answer* brings (`Browser.wait_exists`) before asserting
  on it.
- Select by path through the file's `table[data-diffnote-file=...]`, not by line
  number alone: a folded file's table has line 2 too.
