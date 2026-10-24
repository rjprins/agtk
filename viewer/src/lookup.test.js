import assert from 'node:assert/strict';
import test from 'node:test';

import { answerLocations, historyDirection } from './lookup.js';

const range = (...bounds) => bounds;

test('maps locations to the shown model or to other files', () => {
  const opened = [];
  const locations = answerLocations({
    locations: [
      { inShownText: true, path: '/repo/a.py', startLine: 3, startColumn: 5, endLine: 3, endColumn: 9 },
      { inShownText: false, path: '/repo/b.py', startLine: 1, startColumn: 1, endLine: 1, endColumn: 4 },
      { inShownText: false, path: '/repo/c.py', startLine: 2, startColumn: 1, endLine: 2, endColumn: 2 },
    ],
    files: { '/repo/b.py': 'def b(): pass\n' },
  }, {
    shownUri: 'shown',
    fileUri: (path, text) => {
      opened.push([path, text]);
      return `file:${path}`;
    },
    range,
  });
  assert.deepEqual(locations, [
    { uri: 'shown', range: [3, 5, 3, 9] },
    { uri: 'file:/repo/b.py', range: [1, 1, 1, 4] },
    { uri: 'file:/repo/c.py', range: [2, 1, 2, 2] },
  ]);
  assert.deepEqual(opened, [['/repo/b.py', 'def b(): pass\n'], ['/repo/c.py', null]]);
});

test('skips malformed locations and answers', () => {
  const options = { shownUri: 'shown', fileUri: () => 'file', range };
  assert.deepEqual(answerLocations(null, options), []);
  assert.deepEqual(answerLocations({ error: 'Install pyright' }, options), []);
  assert.deepEqual(answerLocations({
    locations: [
      { inShownText: true, startLine: 0, startColumn: 1, endLine: 1, endColumn: 1 },
      { inShownText: false, startLine: 1, startColumn: 1, endLine: 1, endColumn: 1 },
      null,
    ],
  }, options), []);
});

test('reads back and forward keys', () => {
  assert.equal(historyDirection({ key: 'ArrowLeft', altKey: true }), 'back');
  assert.equal(historyDirection({ key: 'ArrowRight', altKey: true }), 'forward');
  assert.equal(historyDirection({ key: 'BrowserBack' }), 'back');
  assert.equal(historyDirection({ key: 'ArrowLeft', altKey: true, shiftKey: true }), null);
  assert.equal(historyDirection({ key: 'ArrowLeft' }), null);
});
