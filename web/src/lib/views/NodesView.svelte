<script>
  // The fleet: every node that has announced itself.
  //
  // A node announces itself by existing — it emits to the multicast group
  // from the moment its network is up in the initramfs, so a console that
  // joins the group sees every node on the segment with no configuration
  // on either side (CLUSTER.md). There is no inventory service and no
  // registration, which is why this list is built from log traffic.
  //
  // Health here is recency, not a probe: a node that stopped talking is
  // the thing worth noticing, and it is the only thing the log group can
  // honestly tell you. What a node is *running* is a question for its own
  // page, which asks it directly.
  import { feed, prefs, setView } from '../stores.svelte.js'
  import { call } from '../api.js'
  import PageHeader from '../components/PageHeader.svelte'
  import Toolbar from '../components/Toolbar.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import ResourceTable from '../components/ResourceTable.svelte'
  import ComponentCard from 'stormview/components/ComponentCard.svelte'

  let search = $state('')
  let state = $state('')

  const resolveId = (id) => feed.components.find((c) => c.id === id)
  const invoke = (a) => call(a.method, a.path)

  const all = $derived(feed.components.filter((c) => c.kind === 'node'))
  const rows = $derived(
    all.filter((c) => {
      if (state && c.health !== state) return false
      if (!search) return true
      return `${c.label} ${c.detail || ''}`.toLowerCase().includes(search.toLowerCase())
    })
  )
  const quiet = $derived(all.filter((c) => c.health === 'error' || c.health === 'warn').length)
  const collector = $derived(feed.components.find((c) => c.id === 'plugin:logs'))
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Compute' }, { label: 'Nodes' }]}
    title="Nodes"
    count={feed.loaded ? all.length : null}
  >
    {#snippet status()}
      {#if quiet}
        <span class="quiet" title="Nodes that have not been heard from recently">
          {quiet} quiet
        </span>
      {/if}
    {/snippet}
  </PageHeader>

  {#if !feed.loaded}
    <div class="sc-empty"><p>Connecting to the component feed…</p></div>
  {:else if all.length === 0}
    <EmptyState
      icon="node"
      title="No nodes heard"
      hint={collector && collector.health !== 'ok'
        ? `The log collector is not listening, so nothing can announce itself: ${collector.detail}`
        : 'Nothing has announced itself on the multicast group. A node joins it as soon as its network is up, so an empty list means either no node is on this segment or the group is not reaching this console — a bridged or NAT’d container cannot join it.'}
    />
  {:else}
    <Toolbar
      bind:search
      placeholder="Search nodes by name or address"
      bind:view={prefs.view}
      onview={setView}
      hint={rows.length !== all.length ? `${rows.length} of ${all.length}` : `${all.length} heard`}
    >
      {#snippet filters()}
        <select bind:value={state} aria-label="Filter by state">
          <option value="">All nodes</option>
          <option value="ok">Heard recently</option>
          <option value="warn">Going quiet</option>
          <option value="error">Not heard</option>
        </select>
      {/snippet}
    </Toolbar>

    {#if rows.length === 0}
      <EmptyState icon="filter" title="No matches" hint="No node matches the current search and filter." />
    {:else if prefs.view === 'cards'}
      <div class="grid sc-cards">
        {#each rows as c (c.id)}
          <ComponentCard component={c} resolve={resolveId} {invoke} />
        {/each}
      </div>
    {:else}
      <ResourceTable components={feed.components} rootIds={rows.map((c) => c.id)} {invoke} showKind={false} />
    {/if}
  {/if}
</div>

<style>
  .quiet {
    font-size: var(--sc-t-meta);
    color: var(--warn-strong);
    font-variant-numeric: tabular-nums;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 12px;
  }
</style>
