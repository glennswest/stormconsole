import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

// The SPA is served by stormconsole at /. Output filenames are fixed (no
// content hashes) so the embedded-asset handler and the git diff of
// web/dist stay stable across builds.
const target = process.env.STORMCONSOLE_URL || 'http://localhost:9094'

// Zero warnings (#383, stormcentral#193): a build fails on a Svelte compiler warning in our code and on any of
// vite-plugin-svelte's own (a package with a `svelte` field and no `svelte` export condition, as stormview@0.4.0
// was). The plugin prints its own through console.warn as `[vite-plugin-svelte] WARNING: …`, from its config hook.
// Its logger holds the console.warn it found when it was imported (before this file's body runs), so a build
// watches what reaches stderr instead, where every console.warn lands, and records those lines as they pass.
// Compiler warnings come through `onwarn`, which records and prints each of our own (a package's own are left to
// the plugin, which shows them only when debugging, as before), so none is counted twice. Every one is printed,
// and the build ends on them.
const warned = []
if (process.argv.includes('build')) {
  const write = process.stderr.write.bind(process.stderr)
  process.stderr.write = (chunk, ...rest) => {
    const text = String(chunk).replace(/\x1b\[[0-9;]*m/g, '')
    for (const line of text.split('\n')) if (/\[vite-plugin-svelte[^\]]*\].*warn/i.test(line)) warned.push(line.trim())
    return write(chunk, ...rest)
  }
}
const strict = {
  name: 'stormconsole-zero-warnings',
  apply: 'build',
  buildEnd(error) {
    if (!error && warned.length) {
      this.error(`the web build has ${warned.length} warning(s), and it must have none (#383):\n  ${warned.join('\n  ')}`)
    }
  },
}

export default defineConfig({
  base: '/',
  plugins: [
    svelte({
      onwarn(w, handle) {
        if (w.filename?.includes('node_modules')) return handle(w)
        const line = `svelte ${w.code}: ${w.filename}:${w.start?.line ?? '?'}:${w.start?.column ?? '?'} ${w.message.split('\n')[0]}`
        warned.push(line)
        console.error(`warning: ${line}${w.frame ? `\n${w.frame}` : ''}`)
      },
    }),
    strict,
  ],
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
