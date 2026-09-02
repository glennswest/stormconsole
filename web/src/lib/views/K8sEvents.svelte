<script>
  // Cluster events from the k8s plugin, scoped by the namespace selector.
  // Fetched on demand (rustkube events are a list, not a stream here) with
  // a refresh cadence while the view is open.
  import { onMount } from 'svelte'
  import { k8sns } from '../stores.svelte.js'
  import { get, timeAgo } from '../api.js'
  import PageHeader from '../components/PageHeader.svelte'
  import Toolbar from '../components/Toolbar.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import Icon from '../components/Icon.svelte'

  let all = $state([])
  let error = $state('')
  let loaded = $state(false)
  let search = $state('')
  let type = $state('')
  let busy = $state(false)

  const rows = $derived(
    all.filter((e) => {
      if (type && e.type !== type) return false
      if (!search) return true
      const q = search.toLowerCase()
      return `${e.reason} ${e.object} ${e.message}`.toLowerCase().includes(q)
    })
  )
  const warnings = $derived(all.filter((e) => e.type === 'Warning').length)

  async function refresh() {
    busy = true
    try {
      const q = k8sns.selected ? `?namespace=${encodeURIComponent(k8sns.selected)}` : ''
      const list = await get(`/api/plugins/k8s/events${q}`)
      all = Array.isArray(list) ? list.sort((a, b) => (b.time || '').localeCompare(a.time || '')) : []
      error = ''
    } catch (e) {
      error = e.message
    }
    loaded = true
    busy = false
  }

  $effect(() => {
    k8sns.selected
    refresh()
  })

  onMount(() => {
    const t = setInterval(refresh, 10000)
    return () => clearInterval(t)
  })
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Cluster' }, { label: 'Events' }]}
    title="Events"
    scope={k8sns.selected ? `in ${k8sns.selected}` : ''}
    count={loaded && !error ? all.length : null}
  >
    {#snippet actions()}
      <button onclick={refresh} disabled={busy} title="Refresh now">
        <Icon name="refresh" size={14} /> Refresh
      </button>
    {/snippet}
  </PageHeader>

  {#if !loaded}
    <div class="sc-empty"><p>Loading events…</p></div>
  {:else if error}
    <EmptyState
      icon="events"
      title="Events are unavailable"
      hint="The kubernetes plugin returned: {error}"
    >
      {#snippet action()}
        <button class="sc-primary" onclick={refresh}>Try again</button>
      {/snippet}
    </EmptyState>
  {:else if all.length === 0}
    <EmptyState
      icon="events"
      title="No events recorded"
      hint="The cluster has not reported an event in the retention window. Events appear here as controllers act."
    />
  {:else}
    <Toolbar
      bind:search
      placeholder="Search reason, object or message"
      hint={rows.length !== all.length ? `${rows.length} of ${all.length}` : `${warnings} warnings`}
    >
      {#snippet filters()}
        <select bind:value={type} aria-label="Filter by type">
          <option value="">All types</option>
          <option value="Normal">Normal</option>
          <option value="Warning">Warning</option>
        </select>
      {/snippet}
    </Toolbar>

    {#if rows.length === 0}
      <EmptyState icon="filter" title="No matches" hint="No event matches the current search and type filter." />
    {:else}
      <div class="sc-panel table-wrap">
        <table>
          <thead>
            <tr>
              <th class="w-time">Last seen</th>
              <th class="w-type">Type</th>
              <th>Reason</th>
              <th>Object</th>
              <th class="w-ns">Namespace</th>
              <th>Message</th>
              <th class="w-n">Count</th>
            </tr>
          </thead>
          <tbody>
            {#each rows as e}
              <tr class:warn={e.type === 'Warning'}>
                <td class="time" title={e.time}>{e.time ? timeAgo(e.time) : '—'}</td>
                <td>
                  <span class="type" class:warn={e.type === 'Warning'}>{e.type}</span>
                </td>
                <td class="reason">{e.reason}</td>
                <td class="mono">{e.object}</td>
                <td>{e.namespace}</td>
                <td class="msg">{e.message}</td>
                <td class="n">{e.count}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
</div>

<style>
  .table-wrap { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  th {
    text-align: left;
    font-size: var(--sc-t-meta);
    font-weight: 600;
    color: var(--text-dim);
    background: color-mix(in srgb, var(--panel-raised) 55%, var(--panel));
    padding: 8px 14px;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
    position: sticky;
    top: 0;
  }
  td {
    padding: 8px 14px;
    border-bottom: 1px solid var(--sc-hairline);
    color: var(--text);
    vertical-align: top;
  }
  tbody tr:last-child td { border-bottom: none; }
  tbody tr:hover { background: var(--nav-hover); }
  .type {
    font-size: var(--sc-t-eyebrow);
    font-weight: 600;
    padding: 1px 8px;
    border-radius: 999px;
    border: 1px solid var(--border);
    color: var(--text-faint);
    white-space: nowrap;
  }
  .type.warn { color: var(--warn-strong); border-color: var(--warn-border); background: var(--warn-bg); }
  .reason { font-weight: 500; white-space: nowrap; }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .msg { color: var(--text-dim); min-width: 280px; }
  .time { white-space: nowrap; color: var(--text-dim); }
  .n { text-align: right; color: var(--text-dim); }
  .w-time { width: 110px; }
  .w-type { width: 90px; }
  .w-ns { width: 140px; }
  .w-n { width: 64px; text-align: right; }
</style>
