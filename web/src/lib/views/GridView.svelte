<script>
  // A list rooted at one component — where a nav item or a card's ⊞ lands.
  // With ?rel= the top rows are that relationship's targets; without it,
  // the component itself is the single expandable root.
  import { route } from '../router.svelte.js'
  import { feed, prefs, setView, idsForRoute } from '../stores.svelte.js'
  import ResourceTable from '../components/ResourceTable.svelte'
  import ComponentCard from 'stormview/components/ComponentCard.svelte'
  import PageHeader from '../components/PageHeader.svelte'
  import Toolbar from '../components/Toolbar.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import StatusPill from '../components/StatusPill.svelte'
  import CreateMenu from '../components/CreateMenu.svelte'
  import { call } from '../api.js'

  const id = $derived(route.current.query.get('id'))
  const rel = $derived(route.current.query.get('rel'))
  const root = $derived(feed.components.find((c) => c.id === id))
  const hash = $derived(`#/grid?id=${id}${rel ? `&rel=${rel}` : ''}`)

  let search = $state('')
  // What a row *is*, from the metric the feed carries. Generic: any grid
  // whose rows declare a `kind` metric gets the filter, and one whose rows
  // do not gets nothing rather than an empty control.
  let kind = $state('')
  let state_f = $state('')

  const kindOf = (c) =>
    (c.metrics || []).find((m) => m.label === 'kind')?.value || ''

  const kinds = $derived([...new Set(all.map(kindOf).filter(Boolean))].sort())

  const resolveId = (cid) => feed.components.find((c) => c.id === cid)
  const invoke = (a) => call(a.method, a.path)

  // With ?rel=, an absent relation means "none yet" — an honest empty
  // list with a way to create, not the root card standing in for it.
  const all = $derived((idsForRoute(hash) || []).map(resolveId).filter(Boolean))
  const rows = $derived(
    all
      .filter((c) => !kind || kindOf(c) === kind)
      .filter((c) => !state_f || c.health === state_f)
      .filter(
        (c) =>
          !search ||
          `${c.label} ${c.detail || ''}`.toLowerCase().includes(search.toLowerCase())
      )
  )
  const title = $derived(rel ? rel.replace(/_/g, ' ') : root?.label || '')
</script>

<div class="sc-page">
  {#if !feed.loaded}
    <div class="sc-empty"><p>Connecting to the component feed…</p></div>
  {:else if !root}
    <EmptyState
      icon="filter"
      title="Component not found"
      hint="“{id}” is no longer in the feed. It may have been deleted, or its plugin may be down."
    >
      {#snippet action()}
        <a class="sc-back" href="#/">Back to overview</a>
      {/snippet}
    </EmptyState>
  {:else}
    <PageHeader
      crumbs={rel
        ? [{ label: 'Overview', href: '#/' }, { label: root.label, href: `#/grid?id=${encodeURIComponent(root.id)}` }, { label: title }]
        : [{ label: 'Overview', href: '#/' }, { label: root.label }]}
      {title}
      count={all.length}
    >
      {#snippet status()}
        {#if !rel}<StatusPill health={root.health} />{/if}
      {/snippet}
      {#snippet actions()}
        <CreateMenu at={hash} primary={true} />
      {/snippet}
    </PageHeader>

    {#if all.length === 0}
      <EmptyState
        icon="inbox"
        title="No {title} yet"
        hint="{root.label} has nothing under this relationship right now."
      >
        {#snippet action()}
          <CreateMenu at={hash} primary={true} />
        {/snippet}
      </EmptyState>
    {:else}
      <Toolbar
        bind:search
        placeholder="Search {title}"
        bind:view={prefs.view}
        onview={setView}
        hint={rows.length !== all.length ? `${rows.length} of ${all.length}` : `${all.length} items`}
      >
        {#snippet filters()}
          {#if kinds.length > 1}
            <!-- Goldens are the bulk of a volume list and rarely the thing
                 being looked for; what matters is what is in use. -->
            <select bind:value={kind} aria-label="Filter by kind">
              <option value="">All kinds</option>
              {#each kinds as k}
                <option value={k}>{k}</option>
              {/each}
            </select>
          {/if}
          <select bind:value={state_f} aria-label="Filter by state">
            <option value="">All states</option>
            <option value="ok">Ready</option>
            <option value="warn">Degraded</option>
            <option value="error">Failed</option>
            <option value="idle">Idle</option>
          </select>
        {/snippet}
      </Toolbar>
      {#if rows.length === 0}
        <EmptyState icon="filter" title="No matches" hint="Nothing here matches that search." />
      {:else if prefs.view === 'cards'}
        <div class="grid sc-cards">
          {#each rows as c (c.id)}
            <ComponentCard component={c} resolve={resolveId} {invoke} />
          {/each}
        </div>
      {:else}
          <ResourceTable components={feed.components} rootIds={rows.map((c) => c.id)} {invoke} />
      {/if}
    {/if}
  {/if}
</div>

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 12px;
  }
  .sc-back { font-size: var(--sc-t-body); }
</style>
