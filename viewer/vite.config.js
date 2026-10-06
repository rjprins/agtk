import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';

export default defineConfig({
  base: './',
  resolve: {
    alias: [{
      // Monaco vendors DOMPurify; npm overrides cannot replace that copy.
      // https://github.com/microsoft/monaco-editor/issues/5454
      find: './dompurify/dompurify.js',
      replacement: fileURLToPath(new URL('./src/monaco-purify.js', import.meta.url)),
    }],
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    assetsInlineLimit: 0,
  },
  server: {
    host: '127.0.0.1',
    strictPort: true,
  },
  worker: {
    format: 'es',
  },
});
