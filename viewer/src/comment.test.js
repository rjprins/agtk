import assert from 'node:assert/strict';
import test from 'node:test';

import { chooseSession, commentEvent, commentSessions, selectedLines } from './comment.js';

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
