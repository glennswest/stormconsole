<script>
  // The node's metrics (#46, stormcos#64): what ironprom scrapes and what it holds. Targets are where a
  // missing level shows. The page names a level or an object; the queries are on the server, because a
  // query from here could read any project's series.
  import { onDestroy } from 'svelte'
  import { route, navigate } from '../router.svelte.js'
  import { ago } from '../ui/time.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import EntityCharts from '../components/EntityCharts.svelte'

  const q = route.current.query
  const entity = q.get('entity') || ''
  const level = q.get('level') || ''

  let targets = $state(null)
  let chartTitle = $state('')
  let error = $state('')
  let timer = null

  // Its refusals are sentences ("no project x"), and they are what the page has to say.
  async function get(path) {
    const r = await fetch(path)
    const body = await r.json().catch(() => null)
    if (!r.ok) throw new Error(body?.error || `${r.status} ${r.statusText}`)
    return body
  }

  async function load() {
    try {
      targets = await get('/api/plugins/metrics/targets')
      error = ''
    } catch (e) {
      error = e.message
    }
    clearTimeout(timer)
    timer = setTimeout(load, 30000)
  }
  load()
  onDestroy(() => clearTimeout(timer))

  // Down first: this view is for finding what is missing.
  const jobs = $derived([...(targets?.jobs || [])].sort((a, b) => (a.up === a.total) - (b.up === b.total) || a.job.localeCompare(b.job)))
  const down = $derived(jobs.reduce((n, j) => n + j.total - j.up, 0))
  const total = $derived(jobs.reduce((n, j) => n + j.total, 0))

  const seconds = (v) => (v >= 1 ? `${v.toFixed(2)} s` : `${(v * 1000).toFixed(v < 0.01 ? 2 : 0)} ms`)
</script>

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Observe' }, { label: 'Metrics' }]} title={chartTitle || 'Metrics'} scope="this node" />

  <nav class="tabs" aria-label="Metrics sections">
    <button class:active={!entity && !level} onclick={() => navigate('#/metrics')}>Targets</button>
    {#each targets?.levels || [] as l (l.id)}
      <button class:active={level === l.id} onclick={() => navigate(`#/metrics?level=${l.id}`)}>{l.title}</button>
    {/each}
  </nav>

  {#if error && !targets?.read}
    <EmptyState icon="overview" title="The metrics cannot be read" hint={error} />
  {:else if targets && !targets.read}
    <EmptyState icon="overview" title={targets.not_started ? 'ironprom is not started on this node' : 'ironprom did not answer'}
      hint={targets.not_started ? 'Nothing is scraping this node, so there is nothing to chart. The Health and Stats pages read their daemons directly.' : targets.error} />
  {:else if entity || level}
    <EntityCharts query={q.toString()} ontitle={(t) => (chartTitle = t)} />
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
  table.targets { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  table.targets th { text-align: left; color: var(--text-faint); font-size: var(--sc-t-meta); font-weight: 600; padding: 6px 10px 6px 0; border-bottom: 1px solid var(--border); }
  table.targets td { padding: 6px 10px 6px 0; border-bottom: 1px solid var(--sc-hairline); vertical-align: top; }
  .err { color: var(--error); overflow-wrap: anywhere; max-width: 420px; }
  .up { color: var(--ok, inherit); }
  .bad { color: var(--error); }
  .dim { color: var(--text-dim); }
  .mono { font-family: var(--mono, monospace); }
</style>
