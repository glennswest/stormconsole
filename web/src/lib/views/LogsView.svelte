<script>
  // The fleet log viewer: recent events from the collector's ring with
  // host/severity/search filters, and live follow over SSE.
  import { onMount } from 'svelte'
  import { get, timeAgo } from '../api.js'

  const SEVERITIES = [
    { v: '', label: 'All severities' },
    { v: '3', label: 'Error and worse' },
    { v: '4', label: 'Warning and worse' },
    { v: '6', label: 'Info and worse' },
  ]
  const SEV_NAMES = ['EMERG', 'ALERT', 'CRIT', 'ERROR', 'WARN', 'NOTICE', 'INFO', 'DEBUG']

  let rows = $state([])
  let hosts = $state([])
  let host = $state('')
  let minSeverity = $state('')
  let search = $state('')
  let follow = $state(true)
  let loaded = $state(false)
  let error = $state('')
  let es = null
  let tableEl = $state(null)

  function params() {
    const p = new URLSearchParams()
    if (host) p.set('host', host)
    if (minSeverity) p.set('min_severity', minSeverity)
    if (search) p.set('search', search)
    return p
  }

  async function refresh() {
    try {
      const p = params()
      p.set('last', '300')
      rows = await get(`/api/plugins/logs/events?${p}`)
      error = ''
    } catch (e) {
      error = e.message
    }
    loaded = true
    scrollToEnd()
    restream()
  }

  async function loadHosts() {
    try {
      const s = await get('/api/plugins/logs/summary')
      hosts = s.hosts || []
    } catch {}
  }

  function restream() {
    es?.close()
    es = null
    if (!follow) return
    // The live tail: search is applied client-side; host/severity at the source.
    es = new EventSource(`/api/plugins/logs/stream?${params()}`)
    es.onmessage = (m) => {
      try {
        const e = JSON.parse(m.data)
        if (search && !`${e.app} ${e.msg}`.toLowerCase().includes(search.toLowerCase())) return
        rows = [...rows.slice(-999), e]
        scrollToEnd()
      } catch {}
    }
  }

  function scrollToEnd() {
    if (follow) queueMicrotask(() => tableEl?.scrollTo(0, tableEl.scrollHeight))
  }

  function toggleFollow() {
    follow = !follow
    restream()
    scrollToEnd()
  }

  onMount(() => {
    refresh()
    loadHosts()
    const t = setInterval(loadHosts, 30000)
    return () => {
      clearInterval(t)
      es?.close()
    }
  })
</script>

<div class="content">
  <div class="bar">
    <h1>Fleet logs</h1>
    <select bind:value={host} onchange={refresh}>
      <option value="">All hosts</option>
      {#each hosts as h}
        <option value={h.host}>{h.host} ({h.count})</option>
      {/each}
    </select>
    <select bind:value={minSeverity} onchange={refresh}>
      {#each SEVERITIES as s}
        <option value={s.v}>{s.label}</option>
      {/each}
    </select>
    <input
      type="search"
      placeholder="Search message…"
      bind:value={search}
      onchange={refresh}
    />
    <button class:on={follow} onclick={toggleFollow} title="Follow live">
      {follow ? '⏸ Pause' : '▶ Follow'}
    </button>
  </div>

  {#if !loaded}
    <div class="empty">Loading…</div>
  {:else if error}
    <div class="empty">Failed: {error}</div>
  {:else if rows.length === 0}
    <div class="empty">No events yet — the collector is listening.</div>
  {:else}
    <div class="table" bind:this={tableEl}>
      <table>
        <tbody>
          {#each rows as e}
            <tr class={'sev' + Math.min(e.severity, 7)}>
              <td class="time" title={e.ts}>{timeAgo(e.ts)}</td>
              <td class="sev">{SEV_NAMES[e.severity] || e.severity}</td>
              <td class="host">{e.host}</td>
              <td class="app">{e.app}</td>
              <td class="msg">{e.msg}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<style>
  .content { display: flex; flex-direction: column; height: 100%; }
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
    flex-wrap: wrap;
  }
  h1 { font-size: 16px; font-weight: 600; margin-right: 8px; }
  select, input {
    padding: 5px 8px;
    font-size: 12px;
    background: var(--panel-raised);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  input { min-width: 200px; }
  button {
    padding: 5px 12px;
    font-size: 12px;
    background: var(--panel-raised);
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  button.on { color: var(--ok); border-color: var(--ok); }
  .table { flex: 1; overflow-y: auto; border: 1px solid var(--border); border-radius: var(--radius-sm); }
  table { width: 100%; border-collapse: collapse; font-size: 12px; font-family: var(--mono, monospace); }
  td { padding: 3px 8px; border-bottom: 1px solid var(--border); color: var(--text-dim); vertical-align: top; }
  .time { white-space: nowrap; color: var(--text-faint); }
  .host { color: var(--accent); white-space: nowrap; }
  .app { color: var(--text-faint); white-space: nowrap; }
  .msg { width: 100%; word-break: break-word; }
  .sev { font-weight: 600; white-space: nowrap; }
  tr.sev0 td.sev, tr.sev1 td.sev, tr.sev2 td.sev, tr.sev3 td.sev { color: var(--error); }
  tr.sev4 td.sev { color: var(--warn, #f1fa8c); }
  tr.sev7 td.sev { color: var(--text-ghost); }
  .empty { color: var(--text-faint); padding: 40px; text-align: center; }
</style>
