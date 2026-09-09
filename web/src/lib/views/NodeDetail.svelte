<script>
  // One node, asked about itself.
  //
  // Before this, the only thing you could do with a node was read its
  // logs — which is not what a node is. CLUSTER.md's promise is that the
  // console is "a view of real nodes running real services", and that
  // every check is "against the running thing": not an inventory of what
  // a machine might have, but its own daemons answering.
  //
  // Fetched on open, not streamed. A node's services are a dozen HTTP
  // probes; putting twenty nodes' worth of components into the feed the
  // console pushes every two seconds would be thousands of rows nobody is
  // looking at.
  import { route } from '../router.svelte.js'
  import { feed } from '../stores.svelte.js'
  import { get } from '../api.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import StatusPill from '../components/StatusPill.svelte'
  import Icon from '../components/Icon.svelte'

  const host = $derived(route.current.params.host)

  let data = $state(null)
  let error = $state('')
  let loaded = $state(false)
  let busy = $state(false)

  // The node's row in the aggregate — health, recency, log volume. It is
  // live, unlike everything else on this page.
  const component = $derived(
    feed.components.find(
      (c) => c.kind === 'node' && (c.label === host || c.id === `fleet:node:${host}`)
    )
  )
  const isLocal = $derived(component?.id === 'fleet:node:local')

  async function load() {
    busy = true
    try {
      data = await get(`/api/plugins/fleet/nodes/${encodeURIComponent(host)}`)
      error = ''
    } catch (e) {
      error = e.message
      data = null
    }
    loaded = true
    busy = false
  }

  $effect(() => {
    if (host) load()
  })

  // A service's own console, through this origin. The node's address never
  // reaches the browser.
  const open = (s) => `${s.proxy}/api/v1/components`
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Compute' }, { label: 'Nodes', href: '#/nodes' }, { label: host }]}
    title={host}
    scope={data?.addr || ''}
  >
    {#snippet status()}
      {#if component}<StatusPill health={component.health} />{/if}
      {#if isLocal}<span class="here">this node</span>{/if}
    {/snippet}
    {#snippet actions()}
      <a class="sc-btn" href={`#/logs?host=${encodeURIComponent(host)}`}>
        <Icon name="logs" size={13} /> Logs
      </a>
      <button onclick={load} disabled={busy} title="Probe this node again">
        <Icon name="refresh" size={13} /> {busy ? 'Probing…' : 'Refresh'}
      </button>
    {/snippet}
  </PageHeader>

  {#if !loaded}
    <div class="sc-empty"><p>Asking {host} what it is running…</p></div>
  {:else if error}
    <EmptyState icon="node" title="This node cannot be asked" hint={error}>
      {#snippet action()}
        <a class="sc-back" href="#/nodes">Back to nodes</a>
      {/snippet}
    </EmptyState>
  {:else}
    <section class="cards">
      <div class="card">
        <h2>Node</h2>
        <dl>
          <dt>Address</dt>
          <dd class="mono">{data.addr || '—'}</dd>
          <dt>Last heard</dt>
          <dd>{component ? component.detail.split(' · ').pop().replace(/^last seen /, '') : '—'}</dd>
          <dt>Log events</dt>
          <dd class="mono">{component?.metrics?.find((m) => m.label === 'events')?.value ?? '—'}</dd>
          <dt>Services</dt>
          <dd class="mono">{data.services.length}</dd>
        </dl>
      </div>
      <div class="card wide">
        <h2>What answered</h2>
        <p class="note">{data.note}</p>
        {#if data.silent.length}
          <p class="silent">
            Quiet: {data.silent.join(', ')} — a worker runs fewer daemons than a control-plane
            node, so this is a difference worth reading, not a fault.
          </p>
        {/if}
      </div>
    </section>

    {#if data.services.length === 0}
      <EmptyState
        icon="node"
        title="Nothing answered on this node"
        hint={data.note}
      >
        {#snippet action()}
          <a class="sc-back" href={`#/logs?host=${encodeURIComponent(host)}`}>Read its logs</a>
        {/snippet}
      </EmptyState>
    {:else}
      <div class="sc-panel table-wrap">
        <table>
          <thead>
            <tr>
              <th class="w-port">Port</th>
              <th>Service</th>
              <th class="w-status">Status</th>
              <th>Detail</th>
              <th class="w-n">Components</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {#each data.services as s (s.port)}
              <tr>
                <td class="mono port">{s.port}</td>
                <td>
                  <span class="name">{s.name}</span>
                  {#if s.name !== s.expected && !s.name.includes(s.expected)}
                    <!-- The layout said one thing and the daemon says
                         another; the daemon wins and the difference shows. -->
                    <span class="expected">layout says {s.expected}</span>
                  {/if}
                </td>
                <td><StatusPill health={s.health} /></td>
                <td class="detail">{s.detail}</td>
                <td class="mono n">{s.components}</td>
                <td class="acts">
                  <a class="sc-btn" href={open(s)} target="_blank" rel="noreferrer">Feed</a>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
</div>

<style>
  .here {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--accent);
    border: 1px solid var(--border-strong);
    border-radius: 999px;
    padding: 1px 8px;
  }

  .cards {
    display: grid;
    grid-template-columns: minmax(240px, 1fr) 2fr;
    gap: 12px;
    align-items: start;
    margin-bottom: 14px;
  }
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px var(--sc-row-px);
  }
  h2 {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin: 0 0 10px;
  }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 4px 14px; margin: 0; }
  dt { font-size: var(--sc-t-meta); color: var(--text-faint); }
  dd { margin: 0; font-size: var(--sc-t-body); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .note { margin: 0; font-size: var(--sc-t-body); color: var(--text); }
  .silent { margin: 8px 0 0; font-size: var(--sc-t-meta); color: var(--text-dim); }

  .table-wrap { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  th {
    text-align: left;
    font-size: var(--sc-t-meta);
    font-weight: 600;
    color: var(--text-dim);
    background: color-mix(in srgb, var(--panel-raised) 55%, var(--panel));
    padding: var(--sc-row-py) var(--sc-row-px);
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
  }
  td {
    padding: var(--sc-row-py) var(--sc-row-px);
    border-bottom: 1px solid var(--sc-hairline);
    vertical-align: middle;
  }
  tbody tr:last-child td { border-bottom: none; }
  tbody tr:nth-child(even) { background: var(--sc-zebra); }
  tbody tr:hover { background: var(--nav-hover); }
  .port { color: var(--text-dim); }
  .name { font-weight: 500; }
  .expected {
    margin-left: 8px;
    font-size: var(--sc-t-eyebrow);
    color: var(--warn-strong);
  }
  .detail { color: var(--text-dim); }
  .n { text-align: right; }
  .acts { text-align: right; white-space: nowrap; }
  .w-port { width: 70px; }
  .w-status { width: 110px; }
  .w-n { width: 100px; text-align: right; }
  .sc-back { font-size: var(--sc-t-body); }
</style>
