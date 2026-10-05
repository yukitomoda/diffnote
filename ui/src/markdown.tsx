// A comment's text, as the elements it was parsed into by Rust.
import { h } from 'preact';
import { EMOJI } from './emoji.ts';
import { lib } from './lib.ts';
import type { DocElement, DocNode, Token } from './model.ts';
import type { Links } from './state/contexts.ts';
import type { FileRef, LineRef } from './lib.ts';
import { Icon } from './icon.tsx';

// A comment: the nodes of its Markdown (see `src/html/markdown.rs`) as
// elements. Only what is known is drawn, so nothing a comment says can be
// anything but text; a link goes only to http, https or mailto.
var SAFE_LINK = /^(https?:|mailto:)/i;

// How a table's column is set, as the data says it.
var ALIGN = { l: 'left', c: 'center', r: 'right' };

/**
 * A comment's text as elements. `links` is what a place in it can go to (an
 * exported page has one too); `inLink` says we are already inside a link, where
 * another one may not be made.
 */
export function markdown(
  nodes: DocNode[] | undefined,
  links: Links | null,
  inLink?: boolean,
): preact.ComponentChildren {
  return (nodes || []).map(function (node: DocNode, i: number): preact.ComponentChildren {
    if (typeof node === 'string') {
      var text = node;
      // `:+1:` is the thumbs-up (the text as written is kept; it is only
      // shown so).
      text = lib.withShortcodes(EMOJI, text);
      if (!links || inLink) return text;
      // `src/a.ts:10-13` in the text goes to those lines, `src/a.ts` to the file.
      return lib.lineRefs(text, links.has, links.revisions).map(function (piece, j) {
        return typeof piece === 'string' ? piece : refLink(piece, j, links, piece.text);
      });
    }
    var n: DocElement = node;
    var kids = markdown(n.c, links, inLink || n.t === 'a');
    switch (n.t) {
      case 'p': return h('p', { key: i }, kids);
      case 'blank': return h('div', { key: i, class: 'diffnote-blank' });
      case 'h': return h('h' + Math.min(Math.max(n.l || 1, 1), 6), { key: i }, kids);
      case 'quote': return h('blockquote', { key: i }, kids);
      case 'ul': return h('ul', { key: i }, kids);
      case 'ol': return h('ol', { key: i, start: n.start }, kids);
      case 'li': return h('li', { key: i }, kids);
      case 'pre': return h('pre', { key: i }, h('code', null, n.s || ''));
      case 'hr': return h('hr', { key: i });
      case 'em': return h('em', { key: i }, kids);
      case 'strong': return h('strong', { key: i }, kids);
      case 'del': return h('del', { key: i }, kids);
      case 'table': return h('div', { key: i, class: 'diffnote-table' }, h('table', null, ((n.c || []) as DocElement[]).map(function (row, r) {
        var head = row.t === 'thead';
        return h(head ? 'thead' : 'tbody', { key: r }, h('tr', null, ((row.c || []) as DocElement[]).map(function (cell, j) {
          var al = ALIGN[((n.al || [])[j] || '') as keyof typeof ALIGN];
          return h(head ? 'th' : 'td', { key: j, style: al ? 'text-align:' + al : undefined }, markdown(cell.c, links, inLink));
        })));
      })));
      case 'image': {
        // An image of the review, drawn only as an <img>: nothing else is loaded.
        var src = links && links.image ? links.image(n.id || '') : '';
        return src ? h('img', { key: i, class: 'diffnote-image', src: src, alt: n.alt || '' }) : h('span', { key: i }, n.alt || lib.m('ui.image_alt_fallback'));
      }
      case 'file': {
        // Another file of the review: only ever to be saved.
        var name = lib.plainText(n.c).trim() || 'file';
        var href = links && links.file ? links.file(n.id || '', name) : '';
        return href ? h('a', { key: i, class: 'diffnote-attachment', href: href, download: name, rel: 'noopener' }, h(Icon, { name: 'attach' }), ' ', kids) : h('span', { key: i }, kids);
      }
      case 'code': {
        // Code that is a place and nothing else (`src/a.ts`, `src/a.ts:3`)
        // goes there too, as code.
        var code = h('code', { key: i }, n.s || '');
        if (!links || inLink) return code;
        var refs = lib.lineRefs(n.s || '', links.has, links.revisions);
        return refs.length === 1 && typeof refs[0] !== 'string' ? refLink(refs[0], i, links, code) : code;
      }
      case 'br': return h('br', { key: i });
      case 'a':
        return n.href && SAFE_LINK.test(n.href)
          ? h('a', { key: i, href: n.href, target: '_blank', rel: 'noopener noreferrer' }, kids)
          : h('span', { key: i }, kids);
      default: return h('span', { key: i }, kids);
    }
  });
}

// A line of code: its pieces of text, each with the kind of thing it is (a
// plain piece is just its text). The colors are the style's (`.tok-*`).
export function tokens(pieces: Token[] | undefined, changed: [number, number][] | undefined) {
  return lib.markPieces(pieces, changed).map(function (p, i) {
    var cls = (p[0] ? 'tok tok-' + p[0] : '') + (p[2] ? (p[0] ? ' ' : '') + 'diffnote-word' : '');
    return cls ? <span key={i} class={cls}>{p[1]}</span> : p[1];
  });
}

/** A place a comment names, as a link that goes there: lines (in a revision,
 * or the one shown), or a file. */
function refLink(piece: LineRef | FileRef, key: number, links: Links, children: preact.ComponentChildren) {
  if ('kind' in piece) {
    return <a key={key} href="#" class="diffnote-lineref" data-diffnote-fileref={piece.path} title={lib.m('ui.link.here')}
      onClick={function (e) { e.preventDefault(); links.go({ kind: 'file', path: piece.path }); }}>{children}</a>;
  }
  var where = links.current === (piece.rev == null ? links.current : piece.rev - 1) ? lib.m('ui.link.here') : lib.mf('ui.link.revision', { rev: String(piece.rev) });
  if (piece.rev != null && piece.rev < links.revisions) where += lib.m('ui.link.not_latest');
  return <a key={key} href="#" class="diffnote-lineref" data-diffnote-lineref={piece.path + ':' + (piece.side === 'old' ? 'L' : '') + piece.start + '-' + piece.end} title={where}
    onClick={function (e) { e.preventDefault(); links.go(piece); }}>{children}</a>;
}
