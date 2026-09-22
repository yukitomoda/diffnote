// A comment's text, as the elements it was parsed into by Rust.
import { h } from 'preact';
import { EMOJI } from './emoji.ts';
import { lib } from './lib.js';
import { html } from './html.ts';

// A comment: the nodes of its Markdown (see `src/html/markdown.rs`) as
// elements. Only what is known is drawn, so nothing a comment says can be
// anything but text; a link goes only to http, https or mailto.
var SAFE_LINK = /^(https?:|mailto:)/i;

export function markdown(nodes, links, inLink) {
  return (nodes || []).map(function (n, i) {
    if (typeof n === 'string') {
      // `:+1:` is 👍 (the text as written is kept; it is only shown so).
      n = lib.withShortcodes(EMOJI, n);
      if (!links || inLink) return n;
      // `src/a.ts:10-13` in the text goes to those lines.
      return lib.lineRefs(n, links.has, links.revisions).map(function (piece, j) {
        if (typeof piece === 'string') return piece;
        var where = links.current === (piece.rev == null ? links.current : piece.rev - 1) ? lib.m('ui.link.here') : lib.mf('ui.link.revision', { rev: String(piece.rev) });
        if (piece.rev != null && piece.rev < links.revisions) where += lib.m('ui.link.not_latest');
        return html`<a key=${j} href="#" class="diffnote-lineref" data-diffnote-lineref=${piece.path + ':' + (piece.side === 'old' ? 'L' : '') + piece.start + '-' + piece.end} title=${where}
          onClick=${function (e) { e.preventDefault(); links.go(piece); }}>${piece.text}</a>`;
      });
    }
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
      case 'table': return h('div', { key: i, class: 'diffnote-table' }, h('table', null, (n.c || []).map(function (row, r) {
        var head = row.t === 'thead';
        return h(head ? 'thead' : 'tbody', { key: r }, h('tr', null, (row.c || []).map(function (cell, j) {
          var al = { l: 'left', c: 'center', r: 'right' }[(n.al || [])[j]];
          return h(head ? 'th' : 'td', { key: j, style: al ? 'text-align:' + al : undefined }, markdown(cell.c, links, inLink));
        })));
      })));
      case 'image': {
        // An image of the review, drawn only as an <img>: nothing else is loaded.
        var src = links && links.image ? links.image(n.id) : '';
        return src ? h('img', { key: i, class: 'diffnote-image', src: src, alt: n.alt || '' }) : h('span', { key: i }, n.alt || lib.m('ui.image_alt_fallback'));
      }
      case 'file': {
        // Another file of the review: only ever to be saved.
        var name = lib.plainText(n.c).trim() || 'file';
        var href = links && links.file ? links.file(n.id, name) : '';
        return href ? h('a', { key: i, class: 'diffnote-attachment', href: href, download: name, rel: 'noopener' }, '📎 ', kids) : h('span', { key: i }, kids);
      }
      case 'code': return h('code', { key: i }, n.s || '');
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
export function tokens(pieces, changed) {
  return lib.markPieces(pieces, changed).map(function (p, i) {
    var cls = (p[0] ? 'tok tok-' + p[0] : '') + (p[2] ? (p[0] ? ' ' : '') + 'diffnote-word' : '');
    return cls ? html`<span key=${i} class=${cls}>${p[1]}</span>` : p[1];
  });
}
