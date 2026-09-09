<script>
  // Virtual machines: what exists, what is running, and what it is on.
  //
  // Rows are feed components, so the lifecycle buttons are the plugin's
  // actions and this view holds no model of its own. Definitions and
  // instances are listed together because "defined but stopped" and
  // "running" are both answers to "what VMs do I have".
  import { feed, k8sns, prefs, setView, idsForRoute } from '../stores.svelte.js'
  import { call } from '../api.js'
  import PageHeader from '../components/PageHeader.svelte'
  import Toolbar from '../components/Toolbar.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import ResourceTable from '../components/ResourceTable.svelte'
  import ComponentCard from 'stormview/components/ComponentCard.svelte'
  import CreateMenu from '../components/CreateMenu.svelte'

  let search = $state('')
  let state = $state('')

  const resolveId = (id) => feed.components.find((c) => c.id === id)
  const invoke = (a) => call(a.method, a.path)

  const all = $derived((idsForRoute('#/vms') || []).map(resolveId).filter(Boolean))
  const rows = $derived(
    all.filter((c) => {
      if (state && c.health !== state) return false
      if (!search) return true
      return `${c.label} ${c.detail || ''}`.toLowerCase().includes(search.toLowerCase())
    })
  )
  const running = $derived(all.filter((c) => c.kind === 'vm').length)
  const plugin = $derived(feed.components.find((c) => c.id === 'plugin:vm'))
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Virtualization' }, { label: 'Virtual machines' }]}
    title="Virtual machines"
    scope={k8sns.selected ? `in ${k8sns.selected}` : ''}
    count={feed.loaded ? all.length : null}
  >
    {#snippet actions()}
      <CreateMenu at="#/vms" primary={true} />
    {/snippet}
  </PageHeader>

  {#if !feed.loaded}
    <div class="sc-empty"><p>Connecting to the component feed…</p></div>
  {:else if all.length === 0}
    <!-- Two different absences, and they need different answers. -->
    <EmptyState
      icon="pod"
      title={k8sns.selected ? `No virtual machines in ${k8sns.selected}` : 'No virtual machines'}
      hint={plugin?.detail?.includes('not installed')
        ? 'The kubevirt.io resources are not installed on this cluster, so nothing here can carry a VM. They arrive with stormpump’s manifest set.'
        : k8sns.selected
          ? 'Nothing runs in the selected namespace. Switch namespaces in the masthead, or create a VM here.'
          : 'This cluster can run VMs and none has been created. A VM here is a VirtualMachineInstance the kubelet reconciles; create one from a golden.'}
    >
      {#snippet action()}
        <CreateMenu at="#/vms" primary={true} />
      {/snippet}
    </EmptyState>
  {:else}
    <Toolbar
      bind:search
      placeholder="Search virtual machines"
      bind:view={prefs.view}
      onview={setView}
      hint={rows.length !== all.length ? `${rows.length} of ${all.length}` : `${running} running`}
    >
      {#snippet filters()}
        <select bind:value={state} aria-label="Filter by state">
          <option value="">All states</option>
          <option value="ok">Running</option>
          <option value="warn">Starting or unplaced</option>
          <option value="error">Failed</option>
          <option value="idle">Stopped</option>
        </select>
      {/snippet}
    </Toolbar>

    {#if rows.length === 0}
      <EmptyState icon="filter" title="No matches" hint="No virtual machine matches the current search and state filter." />
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
