import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

// The SPA is served by stormconsole at /. Output filenames are fixed (no
// content hashes) so the embedded-asset handler and the git diff of
// web/dist stay stable across builds.
const target = process.env.STORMCONSOLE_URL || 'http://localhost:9094'

export default defineConfig({
  base: '/',
  plugins: [svelte()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: {
      output: {
        entryFileNames: 'assets/app.js',
        chunkFileNames: 'assets/[name].js',
        assetFileNames: 'assets/app[extname]',
      },
    },
  },
  server: {
    proxy: {
      '/api': { target },
      '/ws': { target, ws: true },
    },
  },
})
