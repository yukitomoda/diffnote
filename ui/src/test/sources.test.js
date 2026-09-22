// What the page may not contain, over the files we write ourselves.
//
// This used to be in `src/html.rs`, which checked each script it embedded; now
// that the libraries are bundled in with our own code it can't be: preact
// legitimately writes HTML itself. So the rules live here, next to the code
// they are about, and `src/html.rs` keeps the ones that are true of the whole
// bundle (no `fetch` in an exported page, nothing that needs a module).
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const SRC = fileURLToPath(new URL('..', import.meta.url));

// Everything the page is built from: this directory, but not the tests of it
// (they are not in any bundle, and they name the very things they forbid).
function sources(dir = SRC, found = []) {
  for (const name of readdirSync(dir).sort()) {
    const path = join(dir, name);
    if (name === 'test') continue;
    else if (statSync(path).isDirectory()) sources(path, found);
    else if (/\.(js|ts|tsx)$/.test(name)) found.push([path.slice(SRC.length), readFileSync(path, 'utf8')]);
  }
  return found;
}

test('the page is made of elements, never of HTML text', () => {
  // A comment, or a line of code, can never become markup.
  for (const [name, body] of sources()) {
    for (const banned of ['innerHTML', 'insertAdjacentHTML', 'document.write', 'dangerouslySetInnerHTML']) {
      assert.ok(!body.includes(banned), `${name} uses ${banned}`);
    }
  }
});

test('only api.js talks to the server', () => {
  // So that leaving it out of a bundle (entry-export.js) is all it takes for a
  // page to make no requests at all.
  for (const [name, body] of sources()) {
    if (name === 'api.js') continue;
    for (const banned of ['fetch(', 'XMLHttpRequest', 'new Worker', 'serviceWorker']) {
      assert.ok(!body.includes(banned), `${name} uses ${banned}`);
    }
  }
});

test('an element that loads something is built, not written as markup', () => {
  // `h('img', { src })`, never `` html`<img src=${...}>` ``: an exported page
  // is opened from a file, so what a page loads is checked over its markup
  // (`src/html.rs`), which a template's text would end up in.
  for (const [name, body] of sources()) {
    for (const banned of ['<img', '<link', '<iframe', '<script']) {
      assert.ok(!body.includes(banned + ' '), `${name} writes ${banned} as markup`);
    }
  }
});

test('a script can never end the element it sits in', () => {
  // Every bundle is served inside a `<script>` in the page.
  for (const [name, body] of sources()) {
    assert.ok(!body.includes('</script'), `${name} closes its script element`);
  }
});
