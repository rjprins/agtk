import assert from 'node:assert/strict';
import test from 'node:test';

import { languageForFence, languageForPath } from './language.js';

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

test('languageForFence accepts language names and file extensions', () => {
  assert.equal(languageForFence('rust'), 'rust');
  assert.equal(languageForFence('rs'), 'rust');
  assert.equal(languageForFence('Python title="x.py"'), 'python');
  assert.equal(languageForFence('bash'), 'shell');
  assert.equal(languageForFence('yml'), 'yaml');
  assert.equal(languageForFence('mermaid'), null);
  assert.equal(languageForFence(''), null);
});
