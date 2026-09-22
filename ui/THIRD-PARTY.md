# Bundled libraries

The page carries these, so every `diffnote` binary and every exported HTML file
does too. Their licenses are in `licenses/`.

| package | version | license |
|---|---|---|
| [preact](https://preactjs.com/) (with `preact/hooks`) | 10.24.3 | MIT (`licenses/preact-MIT.txt`) |
| [nanostores](https://github.com/nanostores/nanostores) | 1.5.3 | MIT (`licenses/nanostores-MIT.txt`) |
| [@nanostores/preact](https://github.com/nanostores/preact) | 1.1.0 | MIT (`licenses/nanostores-preact-MIT.txt`) |

The versions are pinned exactly in `package.json`: the page is shipped inside a
binary, so what it is built from should not change without being noticed.
