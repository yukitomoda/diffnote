// Bundles the page into `ui/dist`, which `src/html.rs` embeds in the pages it
// writes (see ui/README.md). Run it through mise, which builds the client
// before anything that carries it: `mise run build`.
//
// Two bundles, because an exported page must not carry the code that talks to
// a server: `entry-export.ts` never imports `api.ts`, so none of it is in that
// bundle at all (a test in `src/html.rs` checks the bundle for it).
import * as esbuild from 'esbuild';
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const shared = {
  bundle: true,
  // The page is a plain script, opened from a file: no modules, no requests.
  format: 'iife',
  target: 'es2020',
  // The markup is JSX, turned into preact's own `h` by esbuild.
  jsx: 'automatic',
  jsxImportSource: 'preact',
  // Names are left alone, so that the page can still be read in the browser's
  // own tools; the comments and the spacing are what makes an export big.
  minifyWhitespace: true,
  minifySyntax: true,
  minifyIdentifiers: false,
  // Text stays as it is written (the page is UTF-8); escaping it would make
  // the bundle bigger and unreadable.
  charset: 'utf8',
  legalComments: 'none',
  logLevel: 'warning',
};

await esbuild.build({
  ...shared,
  entryPoints: { export: 'src/entry-export.ts', serve: 'src/entry-serve.ts' },
  outdir: 'dist',
  banner: {
    js: '/*! diffnote: preact and nanostores (MIT), Material Symbols icons (Apache-2.0). See ui/THIRD-PARTY.md. */',
  },
});

await esbuild.build({
  ...shared,
  entryPoints: ['src/style.css'],
  outfile: 'dist/style.css',
});

// What this was built from, so that build.rs can tell whether the bundle it is
// about to embed still matches the source (it can't build it itself: cargo
// doesn't run node). The tests of the page are left out: they are in no bundle.
function inputs(dir, found = []) {
  for (const name of readdirSync(dir).sort()) {
    const path = join(dir, name);
    if (name === 'test') continue;
    else if (statSync(path).isDirectory()) inputs(path, found);
    else found.push(path.replaceAll('\\', '/'));
  }
  return found;
}

const sum = createHash('sha256');
for (const path of [...inputs('src'), 'build.mjs', 'package.json', 'package-lock.json'].sort()) {
  sum.update(path);
  sum.update('\0');
  sum.update(readFileSync(path));
  sum.update('\0');
}
writeFileSync('dist/.sources', sum.digest('hex') + '\n');
