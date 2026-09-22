# Bundled libraries

The page carries these, so every `diffnote` binary and every exported HTML file
does too. Their licenses are in `licenses/`.

| package | version | license |
|---|---|---|
| [preact](https://preactjs.com/) (with `preact/hooks`) | 10.24.3 | MIT (`licenses/preact-MIT.txt`) |
| [htm](https://github.com/developit/htm) | 3.1.1 | Apache-2.0 (`licenses/htm-Apache-2.0.txt`) |

The versions are pinned exactly in `package.json`: the page is shipped inside a
binary, so what it is built from should not change without being noticed.
