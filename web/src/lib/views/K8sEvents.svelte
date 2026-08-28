<script>
  // Cluster events from the k8s plugin, scoped by the namespace selector.
  // Fetched on demand (rustkube events are a list, not a stream here) with
  // a refresh cadence while the view is open.
  import { onMount } from 'svelte'
  import { k8sns } from '../stores.svelte.js'
  import { get, timeAgo } from '../api.js'

  let rows = $state([])
  let error = $state('')
  let loaded = $state(false)

  async function refresh() {
    try {
      const q = k8sns.selected ? `?namespace=${encodeURIComponent(k8sns.selected)}` : ''
      const list = await get(`/api/plugins/k8s/events${q}`)
      rows = Array.isArray(list) ? list.sort((a, b) => (b.time || '').localeCompare(a.time || '')) : []
      error = ''
    } catch (e) {
      error = e.message
    }
    loaded = true
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

<div class="content">
  <h1>
    Events
    {#if k8sns.selected}<span class="scope">in {k8sns.selected}</span>{/if}
    <button class="refresh" onclick={refresh}>Refresh</button>
  </h1>
  {#if !loaded}
    <div class="empty">Loading…</div>
  {:else if error}
    <div class="empty">Failed to load events: {error}</div>
  {:else if rows.length === 0}
    <div class="empty">No events.</div>
  {:else}
    <table>
      <thead>
        <tr><th>Last seen</th><th>Type</th><th>Reason</th><th>Object</th><th>Namespace</th><th>Message</th><th>Count</th></tr>
      </thead>
      <tbody>
        {#each rows as e}
          <tr class:warn={e.type === 'Warning'}>
            <td class="time">{e.time ? timeAgo(e.time) : '—'}</td>
            <td>{e.type}</td>
            <td>{e.reason}</td>
            <td class="mono">{e.object}</td>
            <td>{e.namespace}</td>
            <td class="msg">{e.message}</td>
            <td>{e.count}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  h1 {
    display: flex;
    align-items: baseline;
    gap: 10px;
    font-size: 16px;
    font-weight: 600;
    margin-bottom: 14px;
  }
  .scope { font-size: 13px; color: var(--text-dim); font-weight: 400; }
  .refresh {
    margin-left: auto;
    padding: 4px 12px;
    font-size: 12px;
    background: var(--panel-raised);
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .refresh:hover { color: var(--text); }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th {
    text-align: left;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-faint);
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
  }
  td { padding: 6px 10px; border-bottom: 1px solid var(--border); color: var(--text-dim); }
  tr.warn td { color: var(--warn, #f1fa8c); }
  .mono { font-family: var(--mono, monospace); font-size: 12px; }
  .msg { max-width: 480px; }
  .time { white-space: nowrap; }
  .empty { color: var(--text-faint); padding: 40px; text-align: center; }
</style>
