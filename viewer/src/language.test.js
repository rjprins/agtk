import assert from 'node:assert/strict';
import test from 'node:test';

import { languageForPath } from './language.js';

test('languageForPath maps source and data extensions case-insensitively', () => {
  assert.equal(languageForPath('src/main.rs'), 'rust');
  assert.equal(languageForPath('server/Auth.PY'), 'python');
  assert.equal(languageForPath('web/widget.TSX'), 'typescript');
  assert.equal(languageForPath('config/agent.YML'), 'yaml');
});

test('languageForPath handles common extensionless files', () => {
  assert.equal(languageForPath('Dockerfile'), 'dockerfile');
  assert.equal(languageForPath('.bashrc'), 'shell');
});

test('languageForPath returns null for unknown paths', () => {
  assert.equal(languageForPath('assets/logo.bin'), null);
  assert.equal(languageForPath(''), null);
});
