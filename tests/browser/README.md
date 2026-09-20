# Browser tests

End-to-end tests of the page (`diffnote export`, `diffnote serve`) in a real
headless Chrome, driven over the DevTools protocol with a small client written
for the purpose (`harness.py`, standard library only). They use real mouse
and keyboard events, so what a person does (pressing a line number, dragging,
Shift+click, Escape) is what is tested.

The reviews they open are made by the real `diffnote` (its `edit`, run with a
fake editor: `fake_editor.py`); nothing is checked in as a fixture.

    cargo build
    python3 -m unittest discover -s tests/browser -v

Needs Python 3, Chrome or Chromium (`CHROME=/path/to/chrome` to name it) and
`unzip`, and a built binary (`DIFFNOTE_BIN=...`, default `target/debug/diffnote`).
A test module is skipped, not failed, when the browser or the binary is missing.

Tips for writing more:

- `Browser.wait(expr)` needs a value that comes back by value: wrap a DOM node
  in `!!`.
- A click applies its change on the page at once (a reply as a faded comment,
  a resolve as the new state), and the server's answer swaps the real thing in:
  wait for what the *answer* brings (`Browser.wait_exists`) before asserting
  on it.
- Select by path through the file's `table[data-diffnote-file=...]`, not by line
  number alone: a folded file's table has line 2 too.
