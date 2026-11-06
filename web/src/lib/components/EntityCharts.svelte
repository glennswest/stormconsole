<script>
  // Charts from ironprom for one object or level (#46, #14). `query` is what the server is asked for, by name:
  // never a PromQL query, which could read any project's series.
  import { onDestroy } from 'svelte'
  import { formatBytes } from '../api.js'
  import LineChart from './LineChart.svelte'

  let { query, ontitle } = $props()

  const WINDOWS = [[900, '15 min'], [3600, '1 hour'], [21600, '6 hours'], [86400, '1 day'], [604800, '1 week']]
  let charts = $state(null)
  let error = $state('')
  let windowSecs = $state(3600)
  let timer = null

  async function load(asked, secs) {
    try {
      const p = new URLSearchParams(asked)
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
    seconds,
    cores: (v) => (v >= 1 ? `${v.toFixed(2)} cores` : `${Math.round(v * 1000)}m`),
  }
  const hasPoints = (c) => c.series.some((s) => s.points.some((p) => p.v !== null))
</script>

{#if error}<p class="bad">{error}</p>{/if}
<div class="row">
  {#if charts?.note}<p class="dim note">{charts.note}</p>{/if}
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
