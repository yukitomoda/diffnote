import test from 'node:test';
import assert from 'node:assert/strict';
import { coveredPercent, differingBlocks } from '../diff/pixels.ts';

// A picture of one color, `w` by `h`.
const flat = (w, h, rgba) => ({ width: w, height: h, data: Array.from({ length: w * h }, () => rgba).flat() });
const withPixel = (p, x, y, rgba) => {
  const data = p.data.slice();
  data.splice((y * p.width + x) * 4, 4, ...rgba);
  return { width: p.width, height: p.height, data };
};
const red = [200, 30, 30, 255];
const blue = [30, 30, 200, 255];

test('the same picture twice differs nowhere', () => {
  const a = flat(20, 12, red);
  assert.deepEqual(differingBlocks(a, flat(20, 12, red), 8, 0), []);
  assert.equal(coveredPercent([], a, a), 0);
});

test('a changed pixel marks its block, and only that one', () => {
  const a = flat(20, 12, red);
  const b = withPixel(a, 9, 10, blue);
  assert.deepEqual(differingBlocks(a, b, 8, 0), [{ x: 8, y: 8, w: 8, h: 4 }], 'the block at the edge is as wide as is left');
  assert.equal(coveredPercent(differingBlocks(a, b, 8, 0), a, b), Math.round((32 / 240) * 100));
});

test('a threshold lets a small change of color pass', () => {
  const a = flat(8, 8, red);
  const b = withPixel(a, 0, 0, [205, 30, 30, 255]);
  assert.equal(differingBlocks(a, b, 8, 0).length, 1);
  assert.equal(differingBlocks(a, b, 8, 10).length, 0);
  assert.equal(differingBlocks(a, withPixel(a, 0, 0, [200, 30, 30, 0]), 8, 10).length, 1, 'transparency counts');
});

test('where only one picture has pixels, they differ', () => {
  const a = flat(4, 3, red);
  const b = flat(6, 2, red);
  assert.deepEqual(differingBlocks(a, b, 8, 0), [{ x: 0, y: 0, w: 6, h: 3 }], 'over the larger extent of both');
  const wide = flat(20, 4, red);
  const blocks = differingBlocks(wide, flat(12, 4, red), 8, 0);
  assert.deepEqual(blocks, [{ x: 8, y: 0, w: 8, h: 4 }, { x: 16, y: 0, w: 4, h: 4 }], 'the part the narrower one lacks');
});
