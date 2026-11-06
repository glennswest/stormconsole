<script>
  // The node's metrics (#46, stormcos#64): what ironprom scrapes and what it holds. Targets are where a
  // missing level shows. The page names a level or an object; the queries are on the server, because a
  // query from here could read any project's series.
  import { onDestroy } from 'svelte'
  import { get, formatBytes } from '../api.js'
  import { route, navigate } from '../router.svelte.js'
  import { ago } from '../ui/time.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import LineChart from '../components/LineChart.svelte'

  const q = route.current.query
  const entity = q.get('entity') || ''
  const level = q.get('level') || ''
  const WINDOWS = [[900, '15 min'], [3600, '1 hour'], [21600, '6 hours'], [86400, '1 day'], [604800, '1 week']]

  let targets = $state(null)
  let charts = $state(null)
  let error = $state('')
  let windowSecs = $state(3600)
  let timer = null

  async function load() {
    try {
      targets = await get('/api/plugins/metrics/targets')
      if (entity || level) {
        const p = new URLSearchParams(q)
        p.set('window', windowSecs)
        charts = await get(`/api/plugins/metrics/charts?${p}`)
      }
      error = ''
    } catch (e) {
      error = e.message
    }
    clearTimeout(timer)
    timer = setTimeout(load, 30000)
  }
  $effect(() => {
    void windowSecs
    load()
  })
  onDestroy(() => clearTimeout(timer))

  // Down first: this view is for finding what is missing.
  const jobs = $derived([...(targets?.jobs || [])].sort((a, b) => (a.up === a.total) - (b.up === b.total) || a.job.localeCompare(b.job)))
  const down = $derived(jobs.reduce((n, j) => n + j.total - j.up, 0))
  const total = $derived(jobs.reduce((n, j) => n + j.total, 0))

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

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Observe' }, { label: 'Metrics' }]} title={charts?.title || 'Metrics'} scope="this node" />

  <nav class="tabs" aria-label="Metrics sections">
    <button class:active={!entity && !level} onclick={() => navigate('#/metrics')}>Targets</button>
    {#each targets?.levels || [] as l (l.id)}
      <button class:active={level === l.id} onclick={() => navigate(`#/metrics?level=${l.id}`)}>{l.title}</button>
    {/each}
  </nav>

  {#if error && !charts && !targets?.read}
    <EmptyState icon="overview" title="The metrics cannot be read" hint={error} />
  {:else if targets && !targets.read}
    <EmptyState icon="overview" title={targets.not_started ? 'ironprom is not started on this node' : 'ironprom did not answer'}
      hint={targets.not_started ? 'Nothing is scraping this node, so there is nothing to chart. The Health and Stats pages read their daemons directly.' : targets.error} />
  {:else if entity || level}
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
  {:else if targets}
    <p class="dim">{total - down} of {total} targets up{down ? `, ${down} down` : ''}. A target that does not serve yet shows here, which is the point: the gap is in the same place as the data.</p>
    <table class="targets">
      <thead><tr><th>Job</th><th>Level</th><th>Target</th><th>Health</th><th>Last scrape</th><th>Took</th><th>Last error</th></tr></thead>
      <tbody>
        {#each jobs as j (j.job)}
          {#each j.targets as t (t.url)}
            <tr class:down={t.health !== 'up'}>
              <td class="mono">{j.job}</td>
              <td>{j.level}</td>
              <td class="mono dim">{t.url}</td>
              <td class={t.health === 'up' ? 'up' : 'bad'}>{t.health}</td>
              <td class="dim">{t.last_scrape && !t.last_scrape.startsWith('1970') ? `${ago(t.last_scrape)} ago` : 'never'}</td>
              <td class="dim">{t.seconds ? seconds(t.seconds) : '—'}</td>
              <td class="err">{t.error || ''}</td>
            </tr>
          {/each}
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .tabs { display: flex; flex-wrap: wrap; gap: 2px; border-bottom: 1px solid var(--border); margin-bottom: 14px; }
  .tabs button { background: none; border: none; border-bottom: 2px solid transparent; color: var(--text-dim); padding: 6px 12px; cursor: pointer; border-radius: 0; }
  .tabs button:hover { color: var(--text); background: var(--nav-hover); }
  .tabs button.active { color: var(--text); border-bottom-color: var(--accent); font-weight: 600; }
  .row { display: flex; align-items: baseline; gap: 12px; margin-bottom: 10px; }
  .window { margin-left: auto; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .note { margin: 0; }
  .charts { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 460px), 1fr)); gap: 12px; }
  .chart { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 10px var(--sc-row-px); min-width: 0; }
  .chart h3 { font-size: var(--sc-t-body); margin: 0 0 8px; font-weight: 600; }
  .empty { padding: 18px 0; overflow-wrap: anywhere; }
  table.targets { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  table.targets th { text-align: left; color: var(--text-faint); font-size: var(--sc-t-meta); font-weight: 600; padding: 6px 10px 6px 0; border-bottom: 1px solid var(--border); }
  table.targets td { padding: 6px 10px 6px 0; border-bottom: 1px solid var(--sc-hairline); vertical-align: top; }
  .err { color: var(--error); overflow-wrap: anywhere; max-width: 420px; }
  .up { color: var(--ok, inherit); }
  .bad { color: var(--error); }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
  .mono { font-family: var(--mono, monospace); }
</style>
