<script>
  // API health on this node (#123, stormcos#458): every declared API's
  // state, latency against its budget, and since when; a picked API's
  // kept changes beneath.
  //
  // The node's answer is PID 1's summary (stormpump#127), which carries
  // its own probes — the storage engine's, the registry's — beside every
  // container's stormd. When the console cannot read it, each stormd was
  // asked instead, and the page says what that misses rather than showing
  // a shorter list as if it were the whole.
  import { onDestroy } from 'svelte'
  import { get } from '../api.js'
  import { route } from '../router.svelte.js'
  import { ago } from '../ui/time.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import StatusPill from '../components/StatusPill.svelte'

  const API = '/api/plugins/health'

  let data = $state(null)
  let error = $state('')
  let search = $state('')
  let timer = null

  async function load() {
    try {
      data = await get(`${API}/snapshot`)
      error = ''
    } catch (e) {
      error = `the console did not answer: ${e.message}`
    }
    clearTimeout(timer)
    timer = setTimeout(load, 5000)
  }
  load()
  onDestroy(() => clearTimeout(timer))

  const snap = $derived(data?.snapshot)
  const rows = $derived(
    (data?.rows || []).filter(
      (r) =>
        !search ||
        [r.service, r.api.api, r.api.state, r.api.container || '', r.api.url, r.api.last_error || '']
          .join(' ')
          .toLowerCase()
          .includes(search.toLowerCase()),
    ),
  )
  const counts = $derived.by(() => {
    const c = { healthy: 0, slow: 0, stalled: 0, down: 0, unknown: 0 }
    for (const r of data?.rows || []) if (!(r.api.running === false)) c[r.api.state] = (c[r.api.state] || 0) + 1
    return c
  })

  // The picked API (?api=<key>) and its kept changes.
  let picked = $state(route.current.query.get('api') || '')
  const pickedRow = $derived((data?.rows || []).find((r) => r.key === picked))
  let changes = $state(null)
  let changesError = $state('')

  async function loadHistory(row) {
    changesError = ''
    const q = new URLSearchParams({ limit: '200' })
    if (row) {
      q.set('process', row.api.process)
      if (row.api.api) q.set('api', row.api.api)
    }
    try {
      changes = await get(`${API}/history?${q}`)
    } catch (e) {
      changes = null
      changesError = e.message
    }
  }
  let historyFor = null
  $effect(() => {
    // Once per pick, and once the row it names is known.
    const key = pickedRow ? pickedRow.key : data ? '' : null
    if (key === null || key === historyFor) return
    historyFor = key
    loadHistory(pickedRow)
  })

  function pick(key) {
    picked = picked === key ? '' : key
    const q = new URLSearchParams(route.current.query)
    if (picked) q.set('api', picked)
    else q.delete('api')
    history.replaceState(null, '', q.toString() ? `#/health?${q}` : '#/health')
  }

  const dur = (s) => {
    if (s === null || s === undefined) return ''
    const d = Math.floor(s / 86400), h = Math.floor(s / 3600) % 24, m = Math.floor(s / 60) % 60
    if (d) return `${d}d ${h}h`
    if (h) return `${h}h ${m}m`
    if (m) return `${m}m ${s % 60}s`
    return `${s}s`
  }
  const ms = (v) => (v === null || v === undefined ? '–' : v)
  const over = (a) =>
    (a.budget_p99_ms != null && a.p99_ms != null && a.p99_ms > a.budget_p99_ms) ||
    (a.budget_p50_ms != null && a.p50_ms != null && a.p50_ms > a.budget_p50_ms)
  const tone = { healthy: 'ok', slow: 'warn', stalled: 'error', down: 'error', unknown: 'unknown' }
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Compute' }, { label: 'API health' }]}
    title="API health"
    scope="this node"
    count={data ? data.rows.length : null}
  >
    {#snippet status()}{#if data}<StatusPill health={data.health} />{/if}{/snippet}
  </PageHeader>

  {#if error}<p class="error">{error}</p>{/if}

  {#if data}
    <p class="lede" class:bad={data.health === 'error'} class:warn={data.health === 'warn'}>{data.sentence}</p>
    <p class="dim small">
      {#if snap.source === 'summary'}
        From PID 1's summary, <span class="mono">{snap.summary_file}</span>, written {dur(snap.summary_age_secs)} ago:
        its own probes and every container's stormd.
      {:else if snap.source === 'stormd'}
        <span class="warn">From each stormd on this node{snap.stormds.length ? ` (${snap.stormds.join(', ')})` : ''}.</span>
        PID 1's summary could not be read ({snap.summary_note}), so {data.missing}.
      {/if}
      {#each snap.notes as n}<br /><span class="warn">{n}</span>{/each}
    </p>

    <div class="band">
      {#each ['stalled', 'down', 'slow', 'healthy', 'unknown'] as s}
        {#if counts[s]}<span class="chip {tone[s]}">{counts[s]} {s}</span>{/if}
      {/each}
    </div>

    {#if !data.rows.length}
      <EmptyState
        icon="health"
        title="No APIs"
        hint={snap.source === 'summary'
          ? 'Nothing on this node declares an API to probe yet: stormcos writes the declarations into each image (stormcos#458).'
          : data.sentence}
      />
    {:else}
      <div class="bar">
        <input bind:value={search} placeholder="Search services, APIs, states, errors" aria-label="Search" />
      </div>
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>State</th><th>Service</th><th>API</th><th>For</th><th>Last</th>
              <th title="seen over the last 20 answers">p50 / p99</th><th>Budget p50 / p99</th><th>Error</th><th>Probed by</th>
            </tr>
          </thead>
          <tbody>
            {#each rows as r (r.key)}
              {@const a = r.api}
              <tr class:alert={r.alerting} class:picked={picked === r.key}>
                <td><StatusPill health={r.health} label={a.running === false ? `${a.state}, not running` : a.state} /></td>
                <td><button class="link" onclick={() => pick(r.key)}>{r.service}</button></td>
                <td class="mono" title={a.url}>{a.api || '—'}</td>
                <td class="num" title={a.since || ''}>{dur(r.for_secs)}</td>
                <td class="num">{ms(a.last_ms)}{a.last_ms != null ? ' ms' : ''}</td>
                <td class="num" class:warn={over(a)}>{ms(a.p50_ms)} / {ms(a.p99_ms)} ms</td>
                <td class="num">{ms(a.budget_p50_ms)} / {ms(a.budget_p99_ms)} ms</td>
                <td class="err">{a.last_error || ''}{#if a.stale && a.reported_state} <span class="dim">(said {a.reported_state})</span>{/if}</td>
                <td class="small">{a.source === 'stormpump' ? 'PID 1' : `stormd${a.container ? ' · ' + a.container : ''}`}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if !rows.length}<p class="dim">Nothing matches the search.</p>{/if}
    {/if}

    {#if pickedRow}
      {@const a = pickedRow.api}
      <section class="card">
        <div class="cardhead">
          <h2>{pickedRow.name}</h2>
          <button onclick={() => pick(pickedRow.key)}>Close</button>
        </div>
        <p class:bad={pickedRow.alerting}>{pickedRow.sentence}</p>
        <ul class="kv">
          <li><span class="k">probe</span> <span class="mono">GET {a.url}</span></li>
          <li><span class="k">latency</span> {pickedRow.latency}</li>
          <li><span class="k">since</span> {a.since || '—'}</li>
          <li><span class="k">last probed</span> {a.last_check ? `${ago(a.last_check)} ago` : 'never'}{#if a.interval_secs}, every {a.interval_secs}s{/if}{#if a.checks != null} · {a.checks} probes{/if}</li>
          <li><span class="k">probed by</span> {a.source === 'stormpump' ? `PID 1, unit ${a.process}` : `stormd in ${a.container || 'its container'}, process ${a.process}`}</li>
        </ul>
      </section>
    {/if}

    <section class="card">
      <div class="cardhead">
        <h2>{pickedRow ? `Changes of ${pickedRow.name}` : 'Changes on this node'}</h2>
        {#if changes}<span class="dim small mono">{changes.dir}</span>{/if}
      </div>
      {#if changesError}<p class="error">{changesError}</p>
      {:else if changes && !changes.available}
        <p class="dim">{changes.note}</p>
      {:else if changes && !changes.changes.length}
        <p class="dim">No change kept{pickedRow ? ' for this API' : ''}.{#if changes.note} {changes.note}{/if}</p>
      {:else if changes}
        {#if changes.note}<p class="warn small">{changes.note}</p>{/if}
        <table>
          <thead><tr><th>When</th><th>Service</th><th>API</th><th>Change</th><th>After</th><th>Latency</th><th>Error</th></tr></thead>
          <tbody>
            {#each changes.changes as c}
              <tr class:alert={c.to === 'stalled' || c.to === 'down'}>
                <td class="num" title={c.ts}>{ago(c.ts)} ago</td>
                <td>{c.process}</td>
                <td class="mono">{c.api}</td>
                <td><span class="dim">{c.from} →</span> <StatusPill health={tone[c.to] || 'unknown'} label={c.to} /></td>
                <td class="num">{dur(c.from_secs)}</td>
                <td class="num">{c.latency_ms != null ? `${c.latency_ms} ms` : '–'}{#if c.p99_ms != null} <span class="dim">p99 {c.p99_ms}</span>{/if}</td>
                <td class="err">{c.error || ''}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </section>
  {/if}
</div>

<style>
  .lede { margin: 0 0 4px; font-weight: 600; }
  .small { font-size: var(--sc-t-meta); }
  .dim { color: var(--text-dim); }
  .warn { color: var(--warn-strong); }
  .error, .bad { color: var(--error); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .band { display: flex; gap: 8px; flex-wrap: wrap; margin: 10px 0 12px; }
  .chip { padding: 2px 10px; border-radius: 999px; border: 1px solid currentColor; font-size: var(--sc-t-meta); font-weight: 600; }
  .chip.ok { color: var(--ok); }
  .chip.warn { color: var(--warn-strong); }
  .chip.error { color: var(--error); background: var(--error-bg); }
  .chip.unknown { color: var(--text-dim); }
  .bar { display: flex; gap: 12px; align-items: center; flex-wrap: wrap; margin-bottom: 12px; }
  .bar > input { width: 320px; max-width: 100%; }
  .table-wrap { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); margin-bottom: 12px; font-size: var(--sc-t-body); }
  th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); padding: 8px 12px; border-bottom: 1px solid var(--border); }
  td { padding: 6px 12px; vertical-align: top; }
  tr + tr > td { border-top: 1px solid var(--sc-hairline); }
  tr.alert td:first-child { box-shadow: inset 3px 0 var(--error); }
  tr.picked td { background: var(--nav-hover); }
  td.num { font-variant-numeric: tabular-nums; font-family: var(--mono); font-size: var(--sc-t-meta); white-space: nowrap; }
  td.err { color: var(--text-dim); font-size: var(--sc-t-meta); max-width: 360px; }
  button.link { background: none; border: 0; padding: 0; color: var(--accent); cursor: pointer; font: inherit; text-align: left; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px var(--sc-row-px); margin-bottom: 12px; }
  .card table { border: 0; }
  .cardhead { display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px; gap: 12px; }
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0; }
  .kv { list-style: none; margin: 0; padding: 0; display: grid; gap: 3px; font-size: var(--sc-t-meta); }
  .kv .k { color: var(--text-faint); font-family: var(--mono); margin-right: 6px; }
</style>
