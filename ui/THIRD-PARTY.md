# Bundled libraries

The page carries these, so every `diffnote` binary and every exported HTML file
does too. Their licenses are in `licenses/`.

| package | version | license |
|---|---|---|
| [preact](https://preactjs.com/) (with `preact/hooks`) | 10.24.3 | MIT (`licenses/preact-MIT.txt`) |

The version is pinned exactly in `package.json`: the page is shipped inside a
binary, so what it is built from should not change without being noticed.
