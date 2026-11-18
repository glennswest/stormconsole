<script>
  // Cluster → Custom resources (#348), as OpenShift's Custom Resource
  // Definitions: every kind the apiserver serves beyond its own, with how
  // many of it you may see. A row opens its instances.
  import DataGrid from 'stormview/components/DataGrid.svelte'
  import { get } from '../api.js'
  import { filterCrds } from '../crgrid.js'
  import PageHeader from '../components/PageHeader.svelte'
  import Toolbar from '../components/Toolbar.svelte'
  import EmptyState from '../components/EmptyState.svelte'

  let items = $state(null)
  let error = $state('')
  let search = $state('')
  let only = $state('')

  async function load() {
    try {
      items = (await get('/api/plugins/k8s/crds')).items || []
      error = ''
    } catch (e) {
      error = e.message
    }
  }
  load()

  const rows = $derived(
    filterCrds(items, search, { only }).map((k) => ({
      id: k.name,
      href: k.href,
      name: k.name,
      kind: k.kind,
      group: k.group,
      versions: (k.versions || []).join(', '),
      scope: k.scope,
      // A count the viewer may not take says why instead of a number.
      count: k.count ?? null,
      countText: k.count ?? (k.forbidden ? 'not yours to list' : '—'),
      columns: k.source === 'aggregated' ? 'aggregated' : k.columns ? `${k.columns}` : 'none',
      established: k.established === null || k.established === undefined ? '—' : k.established ? 'True' : 'False',
    }))
  )

  const columns = [
    { key: 'name', label: 'Name', render: 'mono' },
    { key: 'kind', label: 'Kind' },
    { key: 'group', label: 'Group' },
    { key: 'versions', label: 'Versions' },
    { key: 'scope', label: 'Scope' },
    { key: 'count', label: 'Instances', render: (r) => r.countText },
    { key: 'columns', label: 'Printer columns' },
    { key: 'established', label: 'Established' },
  ]
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Cluster' }, { label: 'Custom resources' }]}
    title="Custom resource definitions"
    count={items ? items.length : null}
  />
  <p class="lead">
    Every kind the apiserver serves beyond its own: the CRDs, and what aggregated groups serve. Open one for its
    instances in a grid with the kind's own printer columns, the same ones <code>sc get</code> and
    <code>oc get</code> print.
  </p>

  {#if error}
    <EmptyState icon="cluster" title="The kinds cannot be read" hint="The kubernetes plugin returned: {error}" />
  {:else if !items}
    <div class="sc-empty"><p>Asking the apiserver…</p></div>
  {:else if !items.length}
    <EmptyState icon="inbox" title="No custom resources" hint="The apiserver serves no CRDs and no aggregated groups." />
  {:else}
    <Toolbar bind:search placeholder="Search by name, group, kind or short name" hint={`${rows.length} of ${items.length}`}>
      {#snippet filters()}
        <select bind:value={only} aria-label="Which kinds">
          <option value="">All kinds</option>
          <option value="storm">storm.io kinds</option>
          <option value="instances">With instances</option>
        </select>
      {/snippet}
    </Toolbar>
    <DataGrid {columns} {rows} onrowclick={(r) => (location.hash = r.href)} />
  {/if}
</div>

<style>
  .lead { margin: 0 0 12px; color: var(--text-dim); font-size: var(--sc-t-meta); max-width: 90ch; }
</style>
