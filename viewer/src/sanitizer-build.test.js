import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { build } from 'vite';

test('the viewer bundles only the pinned DOMPurify version', async () => {
  const viewerRoot = new URL('../', import.meta.url);
  const manifest = JSON.parse(await readFile(new URL('package.json', viewerRoot), 'utf8'));
  const result = await build({
    root: fileURLToPath(viewerRoot),
    configFile: fileURLToPath(new URL('vite.config.js', viewerRoot)),
    logLevel: 'silent',
    build: { write: false, minify: false },
  });
  const outputs = (Array.isArray(result) ? result : [result]).flatMap((bundle) => bundle.output);
  const code = outputs.filter((item) => item.type === 'chunk').map((item) => item.code).join('\n');
  const versions = new Set([...code.matchAll(/DOMPurify ([\d.]+)/g)].map((match) => match[1]));
  assert.deepEqual(versions, new Set([manifest.dependencies.dompurify]));
});
