// A changed picture: what it was and what it is, side by side -- and, when
// asked, where the two differ, pixel by pixel, marked on both.
import { h } from 'preact';
import { useContext, useEffect, useRef, useState } from 'preact/hooks';
import { lib } from '../lib.ts';
import { LinksContext } from '../state/contexts.ts';
import type { FileData } from '../model.ts';
import { coveredPercent, differingBlocks } from './pixels.ts';
import type { Block, Pixels } from './pixels.ts';

/** The pixels of a picture that has loaded, read through a canvas. */
function pixelsOf(img: HTMLImageElement): Pixels | null {
  var canvas = document.createElement('canvas');
  canvas.width = img.naturalWidth;
  canvas.height = img.naturalHeight;
  var ctx = canvas.getContext('2d');
  if (!ctx) return null;
  ctx.drawImage(img, 0, 0);
  try {
    var data = ctx.getImageData(0, 0, canvas.width, canvas.height);
    return { width: data.width, height: data.height, data: data.data };
  } catch {
    return null;
  }
}

/** What comparing the two came to. */
interface Marks {
  blocks: Block[];
  percent: number;
  /** The extent the blocks are over (the larger of the two). */
  width: number;
  height: number;
}

interface SideProps {
  label: string;
  digest: string | undefined;
  alt: string;
  which: 'old' | 'new';
  /** Where the two differ, to draw over this side (its own extent of it). */
  marks: Marks | null;
  onLoaded(which: 'old' | 'new', img: HTMLImageElement | null): void;
}

function Side(props: SideProps) {
  var links = useContext(LinksContext);
  var _d = useState<[number, number] | null>(null);
  var dims = _d[0];
  var setDims = _d[1];
  var src = props.digest ? links.blob(props.digest) : '';
  var canvas = useRef<HTMLCanvasElement | null>(null);
  // The marks are drawn in the picture's own pixels; the canvas is laid over
  // the picture and scaled with it.
  useEffect(function () {
    var el = canvas.current;
    if (!el || !dims) return;
    el.width = dims[0];
    el.height = dims[1];
    var ctx = el.getContext('2d');
    if (!ctx) return;
    ctx.clearRect(0, 0, el.width, el.height);
    if (!props.marks) return;
    var paint = ctx;
    var w = el.width;
    var hh = el.height;
    paint.fillStyle = 'rgba(255, 210, 0, 0.55)';
    props.marks.blocks.forEach(function (k) {
      if (k.x >= w || k.y >= hh) return;
      paint.fillRect(k.x, k.y, Math.min(k.w, w - k.x), Math.min(k.h, hh - k.y));
    });
  }, [props.marks, dims]);
  return <figure class={'diffnote-imagediff__side diffnote-imagediff__side--' + props.which} data-diffnote-image-side={props.which}>
    <figcaption>{props.label}{dims && <small>{lib.mf('ui.file.image_size', { w: String(dims[0]), h: String(dims[1]) })}</small>}</figcaption>
    {!props.digest
      ? <p class="diffnote-imagediff__none">{lib.m('ui.file.image_none')}</p>
      : !src
        ? <p class="diffnote-imagediff__none">{lib.m('ui.file.image_not_embedded')}</p>
        : <span class="diffnote-imagediff__frame">
          {/* (Built, not written as markup: what a page loads is checked over
              its markup, see `sources.test.js`.) */}
          {h('img', {
            class: 'diffnote-image diffnote-imagediff__img',
            src: src,
            alt: props.alt,
            onLoad: function (e: Event) {
              var img = e.currentTarget as HTMLImageElement;
              setDims([img.naturalWidth, img.naturalHeight]);
              props.onLoaded(props.which, img);
            },
          })}
          {props.marks && <canvas class="diffnote-imagediff__marks" ref={canvas} data-diffnote-pixel-marks aria-hidden="true" />}
        </span>}
  </figure>;
}

const BLOCK_SIZES = [4, 8, 16, 32];

/** The two versions of a picture the diff changed (one, if it was added or
 * taken out). Pressing a picture shows it by itself, as one in a comment.
 * 「ピクセルの違いを表示」 compares the two pixel by pixel, in blocks, and
 * marks the blocks that differ on both -- only when asked, being work. */
export function ImageDiff(props: { file: FileData }) {
  var file = props.file;
  var image = file.image || {};
  var both = !!(image.old && image.new);
  var _c = useState(false);
  var comparing = _c[0];
  var setComparing = _c[1];
  var _b = useState(8);
  var block = _b[0];
  var setBlock = _b[1];
  var _t = useState(0);
  var threshold = _t[0];
  var setThreshold = _t[1];
  var _m = useState<Marks | null>(null);
  var marks = _m[0];
  var setMarks = _m[1];
  var _w = useState(false);
  var working = _w[0];
  var setWorking = _w[1];
  var loaded = useRef<{ old: HTMLImageElement | null; new: HTMLImageElement | null }>({ old: null, new: null });
  var _n = useState(0);
  var loads = _n[0];
  var setLoads = _n[1];
  var onLoaded = function (which: 'old' | 'new', img: HTMLImageElement | null) {
    loaded.current[which] = img;
    setLoads(function (n) { return n + 1; });
  };
  // The comparison, redone when it is asked for, or the block or the
  // threshold changes, and only once both pictures are there. Off the
  // event, so the button paints as pressed first.
  useEffect(function () {
    if (!comparing) { setMarks(null); return undefined; }
    var a = loaded.current.old;
    var b = loaded.current.new;
    if (!a || !b) return undefined;
    setWorking(true);
    var stale = false;
    var timer = setTimeout(function () {
      var pa = pixelsOf(a!);
      var pb = pixelsOf(b!);
      if (stale) return;
      setWorking(false);
      if (!pa || !pb) { setMarks(null); return; }
      var blocks = differingBlocks(pa, pb, block, threshold);
      setMarks({ blocks: blocks, percent: coveredPercent(blocks, pa, pb), width: Math.max(pa.width, pb.width), height: Math.max(pa.height, pb.height) });
    }, 0);
    return function () { stale = true; clearTimeout(timer); };
  }, [comparing, block, threshold, loads]);
  return <div class="diffnote-imagediff" data-diffnote-image-diff>
    {both && <div class="diffnote-imagediff__tools">
      <button type="button" class={'diffnote-button' + (comparing ? ' is-current' : '')} data-diffnote-pixel-diff aria-pressed={comparing}
        onClick={function () { setComparing(!comparing); }}>{lib.m(comparing ? 'ui.file.pixel_diff_off' : 'ui.file.pixel_diff_on')}</button>
      {comparing && <>
        <label class="diffnote-imagediff__option">{lib.m('ui.file.pixel_block_label')}
          <select data-diffnote-pixel-block value={String(block)} onChange={function (e) { setBlock(Number(e.currentTarget.value)); }}>
            {BLOCK_SIZES.map(function (n) { return <option key={n} value={String(n)}>{n}px</option>; })}
          </select>
        </label>
        <label class="diffnote-imagediff__option">{lib.m('ui.file.pixel_threshold_label')}
          <input type="range" min="0" max="64" step="1" value={String(threshold)} data-diffnote-pixel-threshold
            onChange={function (e) { setThreshold(Number(e.currentTarget.value)); }} />
          <span>{threshold}</span>
        </label>
        <span class="diffnote-imagediff__note" data-diffnote-pixel-note role="status">
          {working ? lib.m('ui.file.pixel_working')
            : marks ? (marks.blocks.length === 0 ? lib.m('ui.file.pixel_same')
              : lib.mf('ui.file.pixel_differs', { n: String(marks.blocks.length), percent: String(marks.percent) }))
            : ''}
        </span>
      </>}
    </div>}
    <div class="diffnote-imagediff__sides">
      {(image.old || file.change !== 'added') && <Side which="old" label={lib.m('ui.file.image_old')} digest={image.old} alt={file.old_path || file.path} marks={marks} onLoaded={onLoaded} />}
      {(image.new || file.change !== 'deleted') && <Side which="new" label={lib.m('ui.file.image_new')} digest={image.new} alt={file.path} marks={marks} onLoaded={onLoaded} />}
    </div>
  </div>;
}
