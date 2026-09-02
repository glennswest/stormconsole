<script>
  // One kubernetes kind, listed. Rows are feed components, so nesting and
  // actions come from their relations and this view holds no model of its
  // own — it adds the chrome: scope, search, state filter, table or cards.
  import { route } from '../router.svelte.js'
  import { feed, k8sns, nav, prefs, setView, idsForRoute } from '../stores.svelte.js'
  import ResourceTable from '../components/ResourceTable.svelte'
  import ComponentCard from 'stormview/components/ComponentCard.svelte'
  import PageHeader from '../components/PageHeader.svelte'
  import Toolbar from '../components/Toolbar.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import CreateMenu from '../components/CreateMenu.svelte'
  import { call } from '../api.js'

  const titles = {
    pod: 'Pods',
    deploy: 'Deployments',
    sts: 'StatefulSets',
    ds: 'DaemonSets',
    job: 'Jobs',
    cronjob: 'CronJobs',
    svc: 'Services',
    pvc: 'PersistentVolumeClaims',
    node: 'Nodes',
    ns: 'Namespaces',
    netpol: 'Network policies',
    cnp: 'Cilium network policies',
    ccnp: 'Cilium clusterwide policies',
    cep: 'Cilium endpoints',
    cn: 'Cilium nodes',
    cid: 'Cilium identities',
  }
  const namespaced = ['pod', 'deploy', 'sts', 'ds', 'job', 'cronjob', 'svc', 'pvc', 'netpol', 'cnp', 'cep']

  const kind = $derived(route.current.params.kind)
  const title = $derived(titles[kind] || kind)
  const at = $derived(`#/k8s/${kind}`)
  const scoped = $derived(k8sns.selected && namespaced.includes(kind))

  let search = $state('')
  let state = $state('')

  const resolveId = (id) => feed.components.find((c) => c.id === id)
  const invoke = (a) => call(a.method, a.path)

  // The section this kind lives under, so the crumb says where you are
  // rather than repeating the page title.
  const section = $derived(
    nav.sections.find((s) => s.items.some((i) => i.href === at))?.label || 'Cluster'
  )

  const all = $derived((idsForRoute(at) || []).map(resolveId).filter(Boolean))
  const rows = $derived(
    all.filter((c) => {
      if (state && c.health !== state) return false
      if (!search) return true
      const q = search.toLowerCase()
      return `${c.label} ${c.detail || ''}`.toLowerCase().includes(q)
    })
  )
  const filtered = $derived(rows.length !== all.length)
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: section }, { label: title }]}
    {title}
    scope={scoped ? `in ${k8sns.selected}` : ''}
    count={feed.loaded ? all.length : null}
  >
    {#snippet actions()}
      <CreateMenu {at} primary={true} />
    {/snippet}
  </PageHeader>

  {#if !feed.loaded}
    <div class="sc-empty"><p>Connecting to the component feed…</p></div>
  {:else if all.length === 0}
    <EmptyState
      icon="inbox"
      title="No {title.toLowerCase()}{scoped ? ` in ${k8sns.selected}` : ''}"
      hint={scoped
        ? 'Nothing of this kind exists in the selected namespace. Switch namespaces in the masthead, or create one.'
        : 'Nothing of this kind exists in the cluster yet.'}
    >
      {#snippet action()}
        <CreateMenu {at} primary={true} />
      {/snippet}
    </EmptyState>
  {:else}
    <Toolbar
      bind:search
      placeholder="Search {title.toLowerCase()}"
      bind:view={prefs.view}
      onview={setView}
      hint={filtered ? `${rows.length} of ${all.length}` : `${all.length} items`}
    >
      {#snippet filters()}
        <select bind:value={state} aria-label="Filter by state">
          <option value="">All states</option>
          <option value="ok">Ready</option>
          <option value="warn">Degraded</option>
          <option value="error">Failed</option>
          <option value="idle">Idle</option>
        </select>
      {/snippet}
    </Toolbar>

    {#if rows.length === 0}
      <EmptyState
        icon="filter"
        title="No matches"
        hint="No {title.toLowerCase()} match the current search and state filter."
      />
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
</div>

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 12px;
  }
</style>
