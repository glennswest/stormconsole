<script>
  // A shell in a container (#12). The relay goes as the person, so what they may do is the apiserver's
  // to say; this page only says what it said. xterm.js is loaded when a terminal is opened, as noVNC is.
  import '@xterm/xterm/css/xterm.css'
  import { onDestroy } from 'svelte'

  let { ns, name, containers = [], refused = '' } = $props()
  const names = containers.filter((c) => c.state?.state === "running").map((c) => c.name)
  let container = $state(names[0] || containers[0]?.name || '')
  let command = $state('')
  let box = $state(null)
  // idle, connecting, open, ended
  let state_ = $state('idle')
  let result = $state('')
  let term = null
  let fit = null
  let socket = null
  let observer = null

  const STDIN = 0
  const STDOUT = 1
  const STDERR = 2
  const ERROR = 3
  const RESIZE = 4
  const encoder = new TextEncoder()

  function frame(channel, bytes) {
    const out = new Uint8Array(bytes.length + 1)
    out[0] = channel
    out.set(bytes, 1)
    return out
  }

  function sendSize() {
    if (socket?.readyState === 1 && term) {
      socket.send(frame(RESIZE, encoder.encode(JSON.stringify({ Width: term.cols, Height: term.rows }))))
    }
  }

  // The result, in metav1.Status: an exit code in its causes, or the words of a refusal.
  function finished(text) {
    try {
      const s = JSON.parse(text)
      if (s.status === 'Success') return 'The command ended.'
      const code = s.details?.causes?.find((c) => c.reason === 'ExitCode')?.message
      return code !== undefined ? `The command ended with exit code ${code}.` : s.message || text
    } catch {
      return text
    }
  }

  async function connect() {
    result = ''
    state_ = 'connecting'
    close()
    state_ = 'connecting'
    const [{ Terminal }, { FitAddon }] = await Promise.all([import('@xterm/xterm'), import('@xterm/addon-fit')])
    const css = getComputedStyle(document.documentElement)
    term = new Terminal({
      cursorBlink: true,
      fontSize: 13,
      scrollback: 5000,
      theme: { background: css.getPropertyValue('--bg').trim() || '#000', foreground: css.getPropertyValue('--text').trim() || '#ddd' },
    })
    fit = new FitAddon()
    term.loadAddon(fit)
    term.open(box)
    fit.fit()

    const q = new URLSearchParams({ container, command })
    const scheme = location.protocol === 'https:' ? 'wss' : 'ws'
    const url = `${scheme}://${location.host}/api/plugins/k8s/pods/${encodeURIComponent(ns)}/${encodeURIComponent(name)}/exec?${q}`
    const s = new WebSocket(url, ['v4.channel.k8s.io'])
    s.binaryType = 'arraybuffer'
    s.onopen = () => {
      state_ = 'open'
      sendSize()
      term.focus()
    }
    s.onmessage = (e) => {
      const b = new Uint8Array(e.data)
      if (b[0] === STDOUT || b[0] === STDERR) term.write(b.subarray(1))
      else if (b[0] === ERROR) result = finished(new TextDecoder().decode(b.subarray(1)))
    }
    s.onclose = () => {
      if (socket === s) {
        state_ = 'ended'
        socket = null
        if (!result) result = 'The connection closed.'
      }
    }
    socket = s
    term.onData((d) => {
      if (socket?.readyState === 1) socket.send(frame(STDIN, encoder.encode(d)))
    })
    term.onResize(sendSize)
    observer = new ResizeObserver(() => fit?.fit())
    observer.observe(box)
  }

  function close() {
    const s = socket
    socket = null
    s?.close()
    observer?.disconnect()
    observer = null
    term?.dispose()
    term = null
    fit = null
    state_ = 'idle'
  }
  onDestroy(close)
</script>

<section class="card">
  <h2>Terminal</h2>
  {#if refused}
    <p class="dim">{refused}</p>
  {:else}
    <div class="bar">
      <label>Container
        <select bind:value={container} aria-label="Container" disabled={state_ === 'open'}>
          {#each containers as c (c.name)}<option value={c.name}>{c.name}</option>{/each}
        </select>
      </label>
      <label class="cmd">Command
        <input bind:value={command} aria-label="Command" placeholder="a shell: bash, or sh" disabled={state_ === 'open'} />
      </label>
      {#if state_ === 'open'}
        <button onclick={close}>Disconnect</button>
      {:else}
        <button class="sc-primary" disabled={state_ === 'connecting' || !container} onclick={connect}>{state_ === 'ended' ? 'Connect again' : 'Connect'}</button>
      {/if}
    </div>
    <p class="dim small">It runs as you: what you may do here is what the cluster lets you do. Opening one is recorded; what is typed is not. An image with no shell has nothing to connect to: name a command it does have.</p>
    {#if result}<p class="result" role="status">{result}</p>{/if}
  {/if}
  <div class="term" class:shown={state_ !== 'idle'} bind:this={box}></div>
</section>

<style>
  .bar { display: flex; flex-wrap: wrap; gap: 12px; align-items: flex-end; margin-bottom: 8px; }
  .bar label { display: flex; flex-direction: column; gap: 3px; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .bar .cmd { flex: 1; min-width: 200px; }
  .bar input { width: 100%; }
  .term { display: none; height: 420px; padding: 6px; background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius); }
  .term.shown { display: block; }
  .result { margin: 6px 0; overflow-wrap: anywhere; }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
</style>
