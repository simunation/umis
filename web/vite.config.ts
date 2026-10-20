import { defineConfig } from 'vite';

// The UI is served by `umis-cli bootstrap --serve-web web/dist`; during
// `npm run dev` API calls go to the bootstrap given by `?boot=`.
export default defineConfig({
  build: { target: 'es2022', outDir: 'dist', emptyOutDir: true },
  // The proving worker imports the wasm-bindgen module (ES only).
  worker: { format: 'es' },
  server: { port: 5173 },
});
