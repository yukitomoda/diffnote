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

# Bundled artwork

| what | where from | license |
|---|---|---|
| The icon shapes in `src/icon.tsx` | [Material Symbols](https://github.com/google/material-design-icons) (Google LLC), outlined, 24px | Apache-2.0 (`licenses/material-symbols-Apache-2.0.txt`) |

Copied as path data rather than loaded as a font: an exported page is opened
from a file and fetches nothing. Each bundle names this file in its own first
line, so an exported HTML file says where its parts came from.
