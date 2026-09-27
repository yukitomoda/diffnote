// A changed picture: what it was and what it is, side by side.
import { h } from 'preact';
import { useContext, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { LinksContext } from '../state/contexts.ts';
import type { FileData } from '../model.ts';

function Side(props: { label: string; digest: string | undefined; alt: string; which: 'old' | 'new' }) {
  var links = useContext(LinksContext);
  var _d = useState<[number, number] | null>(null);
  var dims = _d[0];
  var setDims = _d[1];
  var src = props.digest ? links.blob(props.digest) : '';
  return <figure class={'diffnote-imagediff__side diffnote-imagediff__side--' + props.which} data-diffnote-image-side={props.which}>
    <figcaption>{props.label}{dims && <small>{lib.mf('ui.file.image_size', { w: String(dims[0]), h: String(dims[1]) })}</small>}</figcaption>
    {!props.digest
      ? <p class="diffnote-imagediff__none">{lib.m('ui.file.image_none')}</p>
      : !src
        ? <p class="diffnote-imagediff__none">{lib.m('ui.file.image_not_embedded')}</p>
        // (Built, not written as markup: what a page loads is checked over
        // its markup, see `sources.test.js`.)
        : h('img', {
            class: 'diffnote-image diffnote-imagediff__img',
            src: src,
            alt: props.alt,
            onLoad: function (e: Event) { var img = e.currentTarget as HTMLImageElement; setDims([img.naturalWidth, img.naturalHeight]); },
          })}
  </figure>;
}

/** The two versions of a picture the diff changed (one, if it was added or
 * taken out). Pressing a picture shows it by itself, as one in a comment. */
export function ImageDiff(props: { file: FileData }) {
  var file = props.file;
  var image = file.image || {};
  return <div class="diffnote-imagediff" data-diffnote-image-diff>
    {(image.old || file.change !== 'added') && <Side which="old" label={lib.m('ui.file.image_old')} digest={image.old} alt={file.old_path || file.path} />}
    {(image.new || file.change !== 'deleted') && <Side which="new" label={lib.m('ui.file.image_new')} digest={image.new} alt={file.path} />}
  </div>;
}
