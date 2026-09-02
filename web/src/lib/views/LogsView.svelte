<script>
  // The fleet log viewer: recent events from the collector's ring with
  // host/severity/search filters, and live follow over SSE. The pane owns
  // the remaining height so the tail always sits at the bottom edge.
  import { onMount } from 'svelte'
  import { get, timeAgo } from '../api.js'
  import { route } from '../router.svelte.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import Icon from '../components/Icon.svelte'

  const SEVERITIES = [
    { v: '', label: 'All severities' },
    { v: '3', label: 'Error and worse' },
    { v: '4', label: 'Warning and worse' },
    { v: '6', label: 'Info and worse' },
  ]
  const SEV_NAMES = ['EMERG', 'ALERT', 'CRIT', 'ERROR', 'WARN', 'NOTICE', 'INFO', 'DEBUG']

  let rows = $state([])
  let hosts = $state([])
  // A node card links here with ?host=; the filter starts on that node.
  let host = $state(route.current.query.get('host') || '')
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

<div class="sc-page logs">
  <PageHeader
    crumbs={[{ label: 'Observe' }, { label: 'Fleet logs' }]}
    title="Fleet logs"
    scope={host ? `from ${host}` : ''}
  >
    {#snippet actions()}
      <button class:following={follow} onclick={toggleFollow} title={follow ? 'Pause the live tail' : 'Follow the live tail'}>
        <span class="pulse" class:on={follow}></span>
        {follow ? 'Following' : 'Paused'}
      </button>
    {/snippet}
  </PageHeader>

  <div class="sc-toolbar">
    <label class="sc-search">
      <Icon name="search" size={14} />
      <input type="search" placeholder="Search message" aria-label="Search message" bind:value={search} onchange={refresh} />
    </label>
    <select bind:value={host} onchange={refresh} aria-label="Filter by host">
      <option value="">All hosts</option>
      {#each hosts as h}
        <option value={h.host}>{h.host} ({h.count})</option>
      {/each}
    </select>
    <select bind:value={minSeverity} onchange={refresh} aria-label="Filter by severity">
      {#each SEVERITIES as s}
        <option value={s.v}>{s.label}</option>
      {/each}
    </select>
    <span class="sc-right">
      <span class="sc-hint">{rows.length} lines</span>
      <button onclick={refresh} title="Reload the buffer"><Icon name="refresh" size={14} /></button>
    </span>
  </div>

  {#if !loaded}
    <div class="sc-empty"><p>Loading the ring buffer…</p></div>
  {:else if error}
    <EmptyState icon="logs" title="The log store is unavailable" hint="The collector returned: {error}">
      {#snippet action()}
        <button class="sc-primary" onclick={refresh}>Try again</button>
      {/snippet}
    </EmptyState>
  {:else if rows.length === 0}
    <EmptyState
      icon="logs"
      title="No log lines yet"
      hint="The collector is listening on the fleet multicast group. Lines appear the moment a node sends one."
    />
  {:else}
    <div class="pane" bind:this={tableEl}>
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
  .logs { display: flex; flex-direction: column; height: 100%; padding-bottom: var(--sc-gutter); }

  .following { color: var(--ok); border-color: var(--ok-border); background: var(--ok-bg); }
  .pulse {
    width: 6px; height: 6px; border-radius: 50%;
    background: var(--text-ghost); display: inline-block; margin-right: 6px;
  }
  .pulse.on { background: var(--ok); animation: blink 1.6s ease-in-out infinite; }
  @keyframes blink { 50% { opacity: 0.25; } }

  .pane {
    flex: 1;
    overflow: auto;
    background: var(--term-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  table { width: 100%; border-collapse: collapse; font-family: var(--mono); font-size: var(--sc-t-meta); }
  td { padding: 2px 10px; border-bottom: 1px solid var(--sc-hairline); color: var(--text-dim); vertical-align: top; }
  tbody tr:hover { background: var(--nav-hover); }
  .time { white-space: nowrap; color: var(--text-ghost); }
  .host { color: var(--accent); white-space: nowrap; }
  .app { color: var(--text-faint); white-space: nowrap; }
  .msg { width: 100%; word-break: break-word; color: var(--text); }
  .sev { font-weight: 700; white-space: nowrap; letter-spacing: 0.03em; }
  tr.sev0 td.sev, tr.sev1 td.sev, tr.sev2 td.sev, tr.sev3 td.sev { color: var(--error); }
  tr.sev4 td.sev { color: var(--warn-strong); }
  tr.sev5 td.sev, tr.sev6 td.sev { color: var(--text-faint); }
  tr.sev7 td.sev { color: var(--text-ghost); }
  /* A failure should be findable by scrolling fast, not by reading. */
  tr.sev0, tr.sev1, tr.sev2, tr.sev3 { background: color-mix(in srgb, var(--error-bg) 45%, transparent); }
</style>
