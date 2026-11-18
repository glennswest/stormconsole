<script>
  // One custom resource's instances (#348): the kind's own printer columns
  // (NAME, the columns, AGE; the wide ones behind "Wide", as `-o wide`),
  // the project filter for a namespaced kind, search, sort, and live
  // updates from the viewer's own watch. A row opens the object.
  import { onDestroy } from 'svelte'
  import DataGrid from 'stormview/components/DataGrid.svelte'
  import { route, navigate } from '../router.svelte.js'
  import { projects, loadProjects, k8sns } from '../stores.svelte.js'
  import { get } from '../api.js'
  import { applyChange, shownColumns, wideCount, matches, toGrid, isCurated } from '../crgrid.js'
  import PageHeader from '../components/PageHeader.svelte'
  import Toolbar from '../components/Toolbar.svelte'
  import EmptyState from '../components/EmptyState.svelte'

  const crd = $derived(route.current.params.crd)
  // The project travels in the URL, as everywhere else (#5); the masthead's
  // selection is the default.
  const project = route.current.query.has('ns') ? route.current.query.get('ns') || '' : k8sns.selected || ''

  let meta = $state(null)
  let rows = $state([])
  let error = $state('')
  let live = $state(false)
  let search = $state('')
  let wide = $state(route.current.query.get('wide') === '1')
  let es = null

  if (!projects.loaded) loadProjects()

  function onMessage(ev) {
    let msg
    try {
      msg = JSON.parse(ev.data)
    } catch {
      return
    }
    if (msg.type === 'error') {
      error = msg.error
      return
    }
    if (msg.type === 'snapshot') {
      meta = { kind: msg.kind, columns: msg.columns, gap: msg.gap }
      live = !!msg.live
      error = ''
    }
    rows = applyChange(rows, msg)
  }

  async function start() {
    const q = project ? `?project=${encodeURIComponent(project)}` : ''
    if (typeof EventSource === 'undefined') {
      try {
        onMessage({ data: JSON.stringify({ type: 'snapshot', ...(await get(`/api/plugins/k8s/cr/${crd}${q}`)) }) })
      } catch (e) {
        error = e.message
      }
      return
    }
    // The stream ends every few minutes and the browser opens it again: a
    // fresh list, and the viewer's rights asked again.
    es = new EventSource(`/api/plugins/k8s/cr/${crd}/watch${q}`)
    es.onmessage = onMessage
    es.onerror = async () => {
      live = false
      // A kind that does not exist answers 404 before any stream: say so.
      if (!meta) {
        try {
          await get(`/api/plugins/k8s/cr/${crd}${q}`)
        } catch (e) {
          error = e.message
          es?.close()
        }
      }
    }
  }
  start()
  onDestroy(() => es?.close())

  const namespaced = $derived(!!meta?.kind?.namespaced)
  const cols = $derived(shownColumns(meta?.columns, { wide, project: namespaced ? project : '' }))
  const shown = $derived(rows.filter((r) => matches(r, cols, search)))
  const grid = $derived(toGrid(crd, cols, shown))
  const title = $derived(meta?.kind?.kind ? `All ${meta.kind.plural}` : crd)

  function setProject(p) {
    const q = new URLSearchParams()
    q.set('ns', p)
    if (wide) q.set('wide', '1')
    navigate(`#/cr/${crd}?${q}`)
  }
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Cluster' }, { label: 'Custom resources', href: '#/crds' }, { label: meta?.kind?.kind || crd }]}
    {title}
    scope={namespaced && project ? `in ${project}` : meta && !namespaced ? 'cluster-scoped' : ''}
    count={meta ? rows.length : null}
  >
    {#snippet status()}
      {#if meta}
        <span class="live" class:on={live} title={live ? 'Following your watch of this kind' : 'Listed again every few seconds'}>
          {live ? '● live' : '○ polling'}
        </span>
      {/if}
    {/snippet}
    {#snippet actions()}
      {#if meta?.kind?.source === 'crd'}
        <a class="sc-back" href={`#/grid?id=${encodeURIComponent(`k8s:crd:${crd}`)}`}>Definition</a>
      {/if}
    {/snippet}
  </PageHeader>

  {#if meta}
    <p class="lead mono">
      {meta.kind.group}/{meta.kind.version} · {meta.kind.kind}{#if meta.kind.short?.length} · short {meta.kind.short.join(', ')}{/if}
      {#if isCurated(crd)} · curated columns{/if}
    </p>
  {/if}
  {#if meta?.gap}
    <p class="note">
      {meta.kind.kind} declares no printer columns, so this shows its name, project, age and first-level status
      fields. A kind ships with printer columns: that gap is its owner's to fill.
    </p>
  {/if}

  {#if error && !meta}
    <EmptyState icon="cluster" title="These cannot be listed" hint={error}>
      {#snippet action()}
        <a class="sc-back" href="#/crds">All custom resources</a>
      {/snippet}
    </EmptyState>
  {:else if !meta}
    <div class="sc-empty"><p>Listing {crd}…</p></div>
  {:else}
    {#if error}<p class="stale">Showing the last answer — {error}</p>{/if}
    <Toolbar bind:search placeholder="Search {meta.kind.plural}" hint={shown.length === rows.length ? `${rows.length} items` : `${shown.length} of ${rows.length}`}>
      {#snippet filters()}
        {#if namespaced}
          <select value={project} onchange={(e) => setProject(e.currentTarget.value)} aria-label="Project">
            <option value="">All projects</option>
            {#each projects.list as p (p.name)}<option value={p.name}>{p.name}</option>{/each}
            {#each projects.system as p (p.name)}<option value={p.name}>{p.name} (system)</option>{/each}
            {#if project && ![...projects.list, ...projects.system].some((p) => p.name === project)}
              <option value={project}>{project}</option>
            {/if}
          </select>
        {/if}
        {#if wideCount(meta.columns)}
          <label class="wide"><input type="checkbox" bind:checked={wide} /> Wide (+{wideCount(meta.columns)})</label>
        {/if}
      {/snippet}
    </Toolbar>
    {#if !rows.length}
      <EmptyState
        icon="inbox"
        title="No {meta.kind.plural}{namespaced && project ? ` in ${project}` : ''}"
        hint="Nothing of this kind exists{namespaced && project ? ' in this project' : ''} that you may see."
      />
    {:else if !shown.length}
      <EmptyState icon="filter" title="No matches" hint="No {meta.kind.plural} match the search." />
    {:else}
      <DataGrid columns={grid.columns} rows={grid.rows} onrowclick={(r) => (location.hash = r.href)} />
    {/if}
  {/if}
</div>

<style>
  .lead { margin: 0 0 10px; color: var(--text-faint); font-size: var(--sc-t-meta); }
  .note, .stale {
    margin: 0 0 12px;
    padding: 7px 12px;
    font-size: var(--sc-t-meta);
    color: var(--text-dim);
    background: var(--panel);
    border: 1px solid var(--border);
    border-left: 3px solid var(--accent);
    border-radius: var(--radius-sm);
  }
  .stale { border-left-color: var(--warn-strong); }
  .live { font-size: var(--sc-t-meta); font-weight: 400; color: var(--text-faint); margin-left: 8px; }
  .live.on { color: var(--ok); }
  .wide { display: inline-flex; align-items: center; gap: 4px; font-size: var(--sc-t-meta); color: var(--text-dim); }
</style>
