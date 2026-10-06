import assert from 'node:assert/strict';
import test from 'node:test';
import { isImageDataUrl } from './image.js';

test('only host-encoded images can be loaded by the image viewer', () => {
  for (const mime of ['png', 'jpeg', 'gif', 'webp', 'svg+xml', 'bmp', 'x-icon', 'avif']) {
    assert.equal(isImageDataUrl(`data:image/${mime};base64,YQ==`), true);
  }
  for (const source of [
    'https://example.com/image.png', 'file:///tmp/image.png', 'agtk-diff://viewer/image.png',
    'data:text/html;base64,YQ==', 'data:image/svg+xml,<svg/>',
    'data:image/png;base64,YQ==" onload="alert(1)', null,
  ]) {
    assert.equal(isImageDataUrl(source), false);
  }
});
