<script>
  // Charts from ironprom for one object or level (#46, #14). `query` is what the server is asked for, by name:
  // never a PromQL query, which could read any project's series. `node`: the node that holds the object, whose
  // ironprom the console asks (#369); unset, this node's.
  import { onDestroy, untrack } from 'svelte'
  import { formatBytes } from '../api.js'
  import LineChart from './LineChart.svelte'
  import { chartQuery, fromWords } from '../charts.js'

  // `window`: the window it opens on, in seconds. `longer`: offer 30 and 90 days, for a counter of rare events
  // that ironprom keeps 90 days (#385, ironprom#53).
  let { query, node = '', ontitle, window: opening = 3600, longer = false } = $props()

  const SHORT = [[900, '15 min'], [3600, '1 hour'], [21600, '6 hours'], [86400, '1 day'], [604800, '1 week']]
  const WINDOWS = $derived(longer ? [...SHORT, [2592000, '30 days'], [7776000, '90 days']] : SHORT)
  let charts = $state(null)
  let error = $state('')
  let windowSecs = $state(untrack(() => opening))
  let timer = null

  async function load(asked, secs) {
    try {
      const p = chartQuery(asked, node)
      p.set('window', secs)
      const r = await fetch(`/api/plugins/metrics/charts?${p}`)
      const body = await r.json().catch(() => null)
      // Its refusals are sentences ("no project x"), and they are what the page has to say.
      if (!r.ok) throw new Error(body?.error || `${r.status} ${r.statusText}`)
      charts = body
      ontitle?.(body.title)
      error = ''
    } catch (e) {
      error = e.message
    }
    clearTimeout(timer)
    timer = setTimeout(() => load(asked, secs), 30000)
  }
  $effect(() => {
    load(query, windowSecs)
  })
  onDestroy(() => clearTimeout(timer))

  const small = (v) => (v >= 100 ? String(Math.round(v)) : v >= 1 ? v.toFixed(1) : v.toFixed(3))
  const seconds = (v) => (v >= 1 ? `${v.toFixed(2)} s` : `${(v * 1000).toFixed(v < 0.01 ? 2 : 0)} ms`)
  const bytes = (v) => (v < 1 ? '0 B' : formatBytes(v))
  const FORMAT = {
    celsius: (v) => `${Math.round(v)} °C`,
    percent: (v) => `${Math.round(v)} %`,
    bytes,
    bytes_per_sec: (v) => `${bytes(v)}/s`,
    per_sec: (v) => `${small(v)}/s`,
    per_min: (v) => `${small(v)}/min`,
    seconds,
    cores: (v) => (v >= 1 ? `${v.toFixed(2)} cores` : `${Math.round(v * 1000)}m`),
    // A host port (#336): link and FEC mode are 1 or 0, speed in Mb/s, optical power in mW.
    updown: (v) => (v >= 0.5 ? 'up' : 'down'),
    flag: (v) => (v >= 0.5 ? 'in use' : 'not'),
    mbps: (v) => (v >= 1000 ? `${(v / 1000).toFixed(v % 1000 ? 1 : 0)} Gb/s` : `${Math.round(v)} Mb/s`),
    mw: (v) => `${v.toFixed(3)} mW`,
    // A count of events (#385): stormraid's integrity counters, charted as the count itself.
    count: (v) => String(Math.round(v)),
  }
  const hasPoints = (c) => c.series.some((s) => s.points.some((p) => p.v !== null))
</script>

{#if error}<p class="bad">{error}</p>{/if}
<div class="row">
  {#if charts}<p class="dim note" data-testid="charts-from">{fromWords(charts.node)}.{charts.note ? ` ${charts.note}` : ''}</p>{/if}
  <label class="window">Window
    <select bind:value={windowSecs} aria-label="Window">
      {#each WINDOWS as [s, word] (s)}<option value={s}>{word}</option>{/each}
    </select>
  </label>
</div>
<div class="charts">
  {#each charts?.charts || [] as c (c.id)}
    <section class="chart" aria-label={c.title}>
      <h3>{c.title}</h3>
      {#if c.error}
        <p class="bad small">{c.error}</p>
      {:else if hasPoints(c)}
        <LineChart series={c.series} from={charts.from} to={charts.to} format={FORMAT[c.unit] || small} label={c.title} />
        {#if c.over}<p class="dim small">{c.over} more not drawn.</p>{/if}
      {:else}
        <p class="dim small empty">{c.note || 'Nothing in this window.'}</p>
      {/if}
    </section>
  {/each}
</div>

<style>
  .row { display: flex; align-items: baseline; gap: 12px; margin-bottom: 10px; }
  .window { margin-left: auto; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .note { margin: 0; }
  .charts { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 460px), 1fr)); gap: 12px; }
  .chart { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 10px var(--sc-row-px); min-width: 0; }
  .chart h3 { font-size: var(--sc-t-body); margin: 0 0 8px; font-weight: 600; }
  .empty { padding: 18px 0; overflow-wrap: anywhere; }
  .bad { color: var(--error); overflow-wrap: anywhere; }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
</style>
