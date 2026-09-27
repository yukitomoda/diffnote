// Where two pictures differ, pixel by pixel, in blocks -- apart from how it
// is drawn (Image.tsx), so that it can be tried without a canvas.

/** A picture's pixels as a canvas gives them: RGBA, row by row. */
export interface Pixels {
  width: number;
  height: number;
  data: Uint8ClampedArray | number[];
}

/** A block of pixels, in a picture's own coordinates. */
export interface Block {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * The blocks of `size` pixels a side, over the larger of the two extents,
 * in which the pictures differ: a pixel differs where any of its channels
 * differs by more than `threshold`, and where only one picture has it.
 */
export function differingBlocks(a: Pixels, b: Pixels, size: number, threshold: number): Block[] {
  var width = Math.max(a.width, b.width);
  var height = Math.max(a.height, b.height);
  var out: Block[] = [];
  for (var y = 0; y < height; y += size) {
    for (var x = 0; x < width; x += size) {
      var w = Math.min(size, width - x);
      var h = Math.min(size, height - y);
      if (blockDiffers(a, b, x, y, w, h, threshold)) out.push({ x: x, y: y, w: w, h: h });
    }
  }
  return out;
}

function blockDiffers(a: Pixels, b: Pixels, x0: number, y0: number, w: number, h: number, threshold: number): boolean {
  for (var y = y0; y < y0 + h; y++) {
    for (var x = x0; x < x0 + w; x++) {
      var inA = x < a.width && y < a.height;
      var inB = x < b.width && y < b.height;
      if (inA !== inB) return true;
      if (!inA) continue;
      var i = (y * a.width + x) * 4;
      var j = (y * b.width + x) * 4;
      for (var c = 0; c < 4; c++) {
        if (Math.abs(a.data[i + c] - b.data[j + c]) > threshold) return true;
      }
    }
  }
  return false;
}

/** How much of the larger extent the blocks cover, in percent (rounded). */
export function coveredPercent(blocks: Block[], a: Pixels, b: Pixels): number {
  var area = Math.max(a.width, b.width) * Math.max(a.height, b.height);
  if (!area) return 0;
  var covered = 0;
  blocks.forEach(function (k) { covered += k.w * k.h; });
  return Math.round((covered / area) * 100);
}
