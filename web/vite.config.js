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
    // noVNC (the graphical console) uses top-level await to probe for a
    // hardware H.264 decoder, which needs a 2022-era baseline. It is a
    // lazily-loaded chunk, so this raises the floor only for browsers
    // that open a VM's framebuffer.
    target: 'es2022',
    // IronRDP's package is one chunk with its WebAssembly inlined, loaded only when Remote desktop is used.
    chunkSizeWarningLimit: 8192,
    rollupOptions: {
      output: {
        entryFileNames: 'assets/app.js',
        // A chunk made only of third-party code (xterm, noVNC, IronRDP) is
        // vendored, and lands under assets/vendor/: the stub check
        // (deploy/check-stubs.sh, stormcentral#1455) reads our code, not
        // upstream's internals (xterm's JoinedCellData throws its own
        // marker for a method the terminal never calls), #366.
        chunkFileNames: (chunk) =>
          chunk.moduleIds.length && chunk.moduleIds.every((id) => id.includes('/node_modules/'))
            ? 'assets/vendor/[name].js'
            : 'assets/[name].js',
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
