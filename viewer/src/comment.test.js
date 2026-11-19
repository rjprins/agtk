import assert from 'node:assert/strict';
import test from 'node:test';

import {
  applyEmacs, chooseSession, commentEvent, commentSessions, emacsCommand, selectedLines,
} from './comment.js';

const sessions = [{ id: 'a', name: 'One' }, { id: 'b', name: 'Two' }];

test('chooses the first wanted session that is still offered', () => {
  assert.equal(chooseSession(sessions, ['gone', 'b', 'a']), 'b');
  assert.equal(chooseSession(sessions, ['', null]), 'a');
  assert.equal(chooseSession([], ['a']), null);
});

test('keeps only well-formed sessions', () => {
  assert.deepEqual(
    commentSessions({ sessions: [...sessions, { id: '' }, { id: 'c' }, null], preferred: 'b' }),
    { sessions, preferred: 'b' },
  );
  assert.deepEqual(commentSessions(null), { sessions: [], preferred: null });
});

test('a selection ending at the start of a line does not cover it', () => {
  assert.deepEqual(
    selectedLines({ startLineNumber: 3, endLineNumber: 5, endColumn: 1 }),
    { startLine: 3, endLine: 4 },
  );
  assert.deepEqual(
    selectedLines({ startLineNumber: 3, endLineNumber: 3, endColumn: 1 }),
    { startLine: 3, endLine: 3 },
  );
  assert.deepEqual(
    selectedLines({ startLineNumber: 3, endLineNumber: 5, endColumn: 2 }),
    { startLine: 3, endLine: 5 },
  );
});

test('leaves out text too large for one event', () => {
  const fields = { commentId: 1, documentId: 'd', sessionId: 'a', comment: 'Why?', startLine: 1, endLine: 2 };
  assert.equal(commentEvent({ ...fields, text: 'short' }).text, 'short');
  const large = commentEvent({ ...fields, text: 'é'.repeat(20_000) });
  assert.equal('text' in large, false);
  assert.equal(large.comment, 'Why?');
});

test('reads Emacs keys', () => {
  const key = (key, mods = {}) => emacsCommand({ key, ctrlKey: false, altKey: false, ...mods });
  assert.equal(key('a', { ctrlKey: true }), 'lineStart');
  assert.equal(key('F', { ctrlKey: true }), 'forward');
  assert.equal(key('b', { altKey: true }), 'wordBack');
  assert.equal(key('Backspace', { altKey: true }), 'killWordBack');
  assert.equal(key('a'), null);
  assert.equal(key('f', { ctrlKey: true, shiftKey: true }), null);
  assert.equal(key('f', { ctrlKey: true, altKey: true }), null);
  assert.equal(key('x', { ctrlKey: true }), null);
});

test('moves and deletes like Emacs', () => {
  const text = 'one two\nthree';
  assert.deepEqual(applyEmacs('lineStart', text, 6), { cursor: 0 });
  assert.deepEqual(applyEmacs('lineEnd', text, 2), { cursor: 7 });
  assert.deepEqual(applyEmacs('lineStart', text, 10), { cursor: 8 });
  assert.deepEqual(applyEmacs('lineEnd', text, 10), { cursor: 13 });
  assert.deepEqual(applyEmacs('down', text, 2), { cursor: 10 });
  assert.deepEqual(applyEmacs('down', text, 10), { cursor: 13 });
  assert.deepEqual(applyEmacs('up', text, 12), { cursor: 4 });
  assert.deepEqual(applyEmacs('up', text, 2), { cursor: 0 });
  assert.deepEqual(applyEmacs('wordForward', text, 0), { cursor: 3 });
  assert.deepEqual(applyEmacs('wordForward', text, 3), { cursor: 7 });
  assert.deepEqual(applyEmacs('wordBack', text, 8), { cursor: 4 });
  assert.deepEqual(applyEmacs('back', text, 0), { cursor: 0 });
  assert.deepEqual(applyEmacs('killLine', text, 4), { from: 4, to: 7 });
  assert.deepEqual(applyEmacs('killLine', text, 7), { from: 7, to: 8 });
  assert.deepEqual(applyEmacs('killWord', text, 3), { from: 3, to: 7 });
  assert.deepEqual(applyEmacs('killWordBack', text, 7), { from: 4, to: 7 });
  assert.deepEqual(applyEmacs('deleteBack', text, 0), { from: 0, to: 0 });
});
