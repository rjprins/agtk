import assert from 'node:assert/strict';
import test from 'node:test';

import { headingSlug, renderMarkdown } from './markdown.js';

test('renderMarkdown marks each block with its first source line', () => {
  const html = renderMarkdown('# Title\n\nSome text\nmore\n\n```rust\nfn main() {}\n```\n');
  assert.match(html, /<h1 data-source-line="1">Title<\/h1>/);
  assert.match(html, /<p data-source-line="3">Some text/);
  assert.match(html, /<code data-source-line="6" class="language-rust">/);
});

test('renderMarkdown renders task list items as disabled checkboxes', () => {
  const html = renderMarkdown('- [ ] open\n- [x] done\n- [link](x.md)\n');
  assert.match(html, /<li class="task-list-item" data-source-line="1"><input type="checkbox" disabled> open/);
  assert.match(html, /<input type="checkbox" disabled checked> done/);
  assert.match(html, /<li data-source-line="3"><a href="x.md">link<\/a>/);
});

test('renderMarkdown supports tables and bare links', () => {
  const html = renderMarkdown('| a | b |\n| - | - |\n| 1 | 2 |\n\nSee https://example.com\n');
  assert.match(html, /<table data-source-line="1">/);
  assert.match(html, /<a href="https:\/\/example.com">https:\/\/example.com<\/a>/);
});

test('headingSlug matches GitHub anchors', () => {
  assert.equal(headingSlug('Getting Started'), 'getting-started');
  assert.equal(headingSlug('What’s new in v1.2?'), 'whats-new-in-v12');
  assert.equal(headingSlug('snake_case & kebab-case'), 'snake_case--kebab-case');
});
