<script>
  // A container's logs, as OpenShift shows them (#69): the latest lines,
  // followed live, and the runs before this one.
  //
  // Three sources, one viewer. "Current" is the apiserver's pods/log,
  // streamed while following. "Previous" is the same with ?previous — the
  // run before this one, which is all the node serves. The rest are the
  // runs the console itself kept, fetched as each one ended, because a
  // crash-looping container's earlier attempts are where the reason
  // usually is and the node's API stops at one (rustkube-node#131).
  import { onDestroy, untrack } from 'svelte'
  import { ago } from '../ui/time.js'

  let { ns, name, containers = [], keep = 5, initial = '' } = $props()

  const MAX_LINES = 5000
  const TAIL = 1000

  // The container to open on, chosen once: the page refreshes its
  // container list every few seconds and that must not move the viewer.
  let container = $state(
    untrack(() => initial || containers.find((c) => c.role === 'container')?.name || containers[0]?.name || '')
  )
  let source = $state('current')
  let follow = $state(true)
  let paused = $state(false)
  let wrap = $state(true)
  let timestamps = $state(false)
  let search = $state('')

  let lines = $state([])
  let pending = []
  let pendingCount = $state(0)
  let error = $state('')
  let status = $state('')
  let box = $state(null)
  let ctrl = null

  const base = $derived(`/api/plugins/k8s/pods/${encodeURIComponent(ns)}/${encodeURIComponent(name)}`)
  const current = $derived(containers.find((c) => c.name === container))
  const runs = $derived([...(current?.runs || [])].reverse())

  function url({ download = false } = {}) {
    if (source.startsWith('run:')) {
      const n = source.slice(4)
      return `${base}/runs/${encodeURIComponent(container)}/${n}${download ? '?download=true' : ''}`
    }
    const q = new URLSearchParams({ container })
    if (source === 'previous') q.set('previous', 'true')
    if (timestamps) q.set('timestamps', 'true')
    if (download) q.set('download', 'true')
    else {
      q.set('tailLines', String(TAIL))
      if (follow && source === 'current') q.set('follow', 'true')
    }
    return `${base}/log?${q}`
  }

  function atBottom() {
    return !box || box.scrollHeight - box.scrollTop - box.clientHeight < 40
  }
  function toBottom() {
    queueMicrotask(() => box && (box.scrollTop = box.scrollHeight))
  }

  function append(newLines) {
    if (paused) {
      pending.push(...newLines)
      pendingCount = pending.length
      return
    }
    const stick = atBottom()
    let next = lines.concat(newLines)
    if (next.length > MAX_LINES) next = next.slice(next.length - MAX_LINES)
    lines = next
    if (stick) toBottom()
  }

  function resume() {
    paused = false
    const p = pending
    pending = []
    pendingCount = 0
    append(p)
    toBottom()
  }

  async function load() {
    ctrl?.abort()
    const c = new AbortController()
    ctrl = c
    lines = []
    pending = []
    pendingCount = 0
    error = ''
    status = 'loading'
    if (!container) return
    try {
      const resp = await fetch(url(), { signal: c.signal })
      if (!resp.ok) {
        const data = await resp.json().catch(() => ({}))
        error = data.error || `${resp.status} ${resp.statusText}`
        status = ''
        return
      }
      const streaming = follow && source === 'current'
      status = streaming ? 'following' : 'loaded'
      const reader = resp.body.getReader()
      const dec = new TextDecoder()
      let rest = ''
      for (;;) {
        const { value, done } = await reader.read()
        if (done) break
        rest += dec.decode(value, { stream: true })
        const parts = rest.split('\n')
        rest = parts.pop()
        if (parts.length) append(parts)
      }
      if (rest) append([rest])
      if (ctrl === c) status = streaming ? 'ended' : 'loaded'
      toBottom()
    } catch (e) {
      if (e.name !== 'AbortError') {
        error = e.message
        status = ''
      }
    }
  }

  // Reload whenever what is being looked at changes — and only then:
  // load() writes state, so it runs untracked or it would re-trigger
  // itself (the loop #56 was).
  $effect(() => {
    void container
    void source
    void follow
    void timestamps
    untrack(load)
  })

  onDestroy(() => ctrl?.abort())

  const needle = $derived(search.trim().toLowerCase())
  const matches = $derived(needle ? lines.filter((l) => l.toLowerCase().includes(needle)).length : 0)

  // A line cut around each match, so the match can be marked without
  // putting log text through {@html}.
  function parts(line) {
    if (!needle) return [{ t: line }]
    const out = []
    const low = line.toLowerCase()
    let i = 0
    for (;;) {
      const j = low.indexOf(needle, i)
      if (j < 0) break
      if (j > i) out.push({ t: line.slice(i, j) })
      out.push({ t: line.slice(j, j + needle.length), m: true })
      i = j + needle.length
    }
    if (i < line.length) out.push({ t: line.slice(i) })
    return out
  }

  function runLabel(r) {
    const missed = r.missed ? ` · ${r.missed} missed before it` : ''
    return `run ${r.run} · ended ~${ago(r.keptAt)} ago${r.truncated ? ' · end only' : ''}${missed}`
  }
</script>

<div class="logs">
  <div class="bar">
    <label>
      Container
      <select bind:value={container} aria-label="Container" onchange={() => (source = 'current')}>
        {#each containers as c (c.name)}
          <option value={c.name}>{c.name}{c.role !== 'container' ? ` (${c.role})` : ''}</option>
        {/each}
      </select>
    </label>
    <label>
      Run
      <select bind:value={source} aria-label="Which run">
        <option value="current">Current</option>
        <option value="previous" disabled={!current?.restartCount}>Previous (from the node)</option>
        {#each runs as r (r.run)}
          <option value={`run:${r.run}`}>{runLabel(r)}</option>
        {/each}
      </select>
    </label>
    <input class="search" type="search" placeholder="Search" bind:value={search} aria-label="Search the log" />
    {#if needle}<span class="dim">{matches} {matches === 1 ? 'line' : 'lines'}</span>{/if}
    <span class="spacer"></span>
    <label class="check"><input type="checkbox" bind:checked={follow} disabled={source !== 'current'} /> Follow</label>
    {#if source === 'current' && follow}
      {#if paused}
        <button onclick={resume}>Resume{pendingCount ? ` (${pendingCount} new)` : ''}</button>
      {:else}
        <button onclick={() => (paused = true)}>Pause</button>
      {/if}
    {/if}
    <label class="check"><input type="checkbox" bind:checked={wrap} /> Wrap</label>
    <label class="check" title="The node adds them; a stormpump container's log has none to add">
      <input type="checkbox" bind:checked={timestamps} disabled={source.startsWith('run:')} /> Timestamps
    </label>
    <a class="btn" href={url({ download: true })} download>Download</a>
  </div>

  <p class="about">
    {#if source === 'current'}
      The last {TAIL} lines of the run now going{follow ? ', followed as it writes' : ''}.
    {:else if source === 'previous'}
      The run before this one, as the node serves it.
    {:else}
      Kept by this console when the run ended. It keeps the last {keep} per container, in memory — a
      console restart starts over, and a run that ended and was replaced between two looks is missed.
    {/if}
    {#if status === 'following'}<span class="live">live</span>{/if}
    {#if status === 'ended'}<span class="dim">— the stream ended (the container stopped, or the node closed it)</span>{/if}
  </p>

  {#if error}
    <p class="err">{error}</p>
  {:else}
    <pre class="out" class:wrap bind:this={box} aria-label="Log output">{#each lines as line, i (i)}<span class="line" class:hit={needle && line.toLowerCase().includes(needle)}>{#each parts(line) as p}{#if p.m}<mark>{p.t}</mark>{:else}{p.t}{/if}{/each}
</span>{:else}<span class="dim">{status === 'loading' ? 'Loading…' : 'No output.'}</span>{/each}</pre>
  {/if}
</div>

<style>
  .logs { display: grid; gap: 8px; }
  .bar { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; }
  .bar label { display: flex; align-items: center; gap: 6px; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .bar .check { gap: 4px; }
  .search { min-width: 180px; }
  .spacer { flex: 1; }
  .btn {
    padding: 4px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text);
    font-size: var(--sc-t-meta);
  }
  .btn:hover { background: var(--nav-hover); text-decoration: none; }
  .about { margin: 0; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .live {
    margin-left: 6px;
    font-size: var(--sc-t-eyebrow);
    color: var(--ok, #3fb950);
    border: 1px solid currentColor;
    border-radius: 999px;
    padding: 0 6px;
  }
  .dim { color: var(--text-faint); font-size: var(--sc-t-meta); }
  .err { color: var(--error, #f85149); font-size: var(--sc-t-body); margin: 0; }
  .out {
    margin: 0;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 8px 10px;
    font-family: var(--mono);
    font-size: 12px;
    line-height: 1.45;
    height: 60vh;
    overflow: auto;
    white-space: pre;
  }
  .out.wrap { white-space: pre-wrap; word-break: break-all; }
  .line.hit { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  mark { background: color-mix(in srgb, var(--accent) 45%, transparent); color: inherit; border-radius: 2px; }
</style>
