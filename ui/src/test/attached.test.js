import test from 'node:test';
import assert from 'node:assert/strict';
import { lib } from '../lib.ts';
import { fileNameOf, inOrder, nameOf } from '../screen/attached.ts';

const ID = 'abcdef0123456789abcdef';
const image = (extra) => Object.assign({ id: ID, kind: 'image', media_type: 'image/png', size: 10 }, extra);
const file = (extra) => Object.assign({ id: ID, kind: 'file', size: 10 }, extra);
const use = (name) => ({ thread: 't', comment: 'c', name });

test('what uses an attachment is read from every comment, with what each calls it', () => {
  const doc = (...nodes) => ({ comments: [{ id: 'c1', doc: nodes }] });
  const threads = [
    Object.assign({ id: 't1' }, doc({ t: 'p', c: [{ t: 'image', id: 'img', alt: '画面' }] })),
    Object.assign({ id: 't2' }, doc({ t: 'p', c: [{ t: 'file', id: 'log', c: ['run.log'] }, { t: 'image', id: 'img', alt: '' }] })),
  ];
  const uses = lib.attachmentUses(threads);
  assert.deepEqual(uses.img, [{ thread: 't1', comment: 'c1', name: '画面' }, { thread: 't2', comment: 'c1', name: '' }]);
  assert.deepEqual(uses.log, [{ thread: 't2', comment: 'c1', name: 'run.log' }]);
  assert.deepEqual(lib.attachmentUses(undefined), {});
});

test('unused attachments come first, and otherwise the order is kept', () => {
  const listed = [{ id: 'big' }, { id: 'unused' }, { id: 'small' }, { id: 'also' }];
  const uses = { big: [use('')], small: [use('')] };
  assert.deepEqual(inOrder(listed, uses).map((a) => a.id), ['unused', 'also', 'big', 'small']);
});

test('the name kept with it wins, then what a comment calls it', () => {
  assert.equal(nameOf(image({ name: 'shot.png' }), { [ID]: [use('説明')] }), 'shot.png');
  assert.equal(nameOf(image(), { [ID]: [use(''), use('説明')] }), '説明');
  assert.equal(nameOf(image(), {}), null);
});

test('it is saved by its own name, a file by its link, and an image without one by its digest', () => {
  assert.equal(fileNameOf(file({ name: 'a.zip' }), {}), 'a.zip');
  assert.equal(fileNameOf(file(), { [ID]: [use('run.log')] }), 'run.log');
  assert.equal(fileNameOf(image(), { [ID]: [use('画面の説明')] }), 'diffnote-abcdef012345.png',
    "an image's text is a description, not a name");
  assert.equal(fileNameOf(image({ media_type: 'image/svg+xml' }), {}), 'diffnote-abcdef012345.svg');
  assert.equal(fileNameOf(file(), {}), 'diffnote-abcdef012345');
});
