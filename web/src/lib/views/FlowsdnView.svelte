<script>
  // The pod network on a flowsdn-edition node (#83, flowsdn#297): what the
  // agent on *this* node says. Its API listens only on the node's loopback,
  // so another node's agent is that node's console's to show.
  //
  // Endpoints lead with the pod they belong to; the numeric identity is a
  // column, not the name. Node and namespace are filters, because a view
  // that silently shows one slice reads as the whole. Booleans are marks
  // (✓ and ·) so they scan down a long list. The rest is what the agent
  // believes: its pools, its modules, its config, the Services it
  // programs, its node routes and the health table. Flows wait on flowsdn
  // (no Hubble observer yet, flowsdn#293), and the tab says so.
  import { onDestroy } from 'svelte'
  import { get } from '../api.js'
  import { route } from '../router.svelte.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import StatusPill from '../components/StatusPill.svelte'
  import CopyButton from '../components/CopyButton.svelte'

  const API = '/api/plugins/flowsdn'
  const TABS = ['Endpoints', 'IPAM', 'Health', 'Config', 'Services', 'Routes', 'State', 'Flows']

  let data = $state(null)
  let error = $state('')
  let tab = $state(TABS.find((t) => t.toLowerCase() === (route.current.query.get('tab') || '').toLowerCase()) || 'Endpoints')
  let node = $state('')
  let ns = $state(route.current.query.get('ns') || '')
  let search = $state('')
  let timer = null

  async function load() {
    try {
      data = await get(`${API}/snapshot`)
      error = ''
    } catch (e) {
      error = `the console did not answer: ${e.message}`
    }
    clearTimeout(timer)
    timer = setTimeout(load, 5000)
  }
  load()
  onDestroy(() => clearTimeout(timer))

  const snap = $derived(data?.snapshot)
  const cilium = $derived(snap?.edition?.name === 'cilium')
  const endpoints = $derived(snap?.endpoints || [])
  const nodes = $derived([...new Set(endpoints.map((e) => e.node).filter(Boolean))].sort())
  const namespaces = $derived([...new Set(endpoints.map((e) => e.namespace).filter(Boolean))].sort())
  const shown = $derived(
    endpoints.filter(
      (e) =>
        (!node || e.node === node) &&
        (!ns || e.namespace === ns) &&
        (!search ||
          [e.namespace, e.pod, e.node, e.state, e.interface, e.attachment, ...e.ipv4, ...e.ipv6, ...e.workloads, String(e.identity ?? '')]
            .join(' ')
            .toLowerCase()
            .includes(search.toLowerCase())),
    ),
  )
  const services = $derived(
    (snap?.services || []).filter((s) => (!ns || s.namespace === ns) && (!search || `${s.namespace}/${s.name} ${s.frontend} ${s.kind}`.toLowerCase().includes(search.toLowerCase()))),
  )

  // One endpoint, read live from the agent with its link health.
  let open = $state(null)
  let openError = $state('')
  async function show(id) {
    openError = ''
    try {
      open = await get(`${API}/endpoint/${encodeURIComponent(id)}`)
    } catch (e) {
      open = null
      openError = `endpoint ${id}: ${e.message}`
    }
  }
  $effect(() => {
    const ep = route.current.query.get('ep')
    if (ep) {
      tab = 'Endpoints'
      show(ep)
    }
  })

  // The health table, asked for by name.
  let table = $state(null)
  let tableError = $state('')
  async function loadTable(name) {
    tableError = ''
    try {
      table = await get(`${API}/state/${encodeURIComponent(name)}`)
    } catch (e) {
      tableError = e.message
    }
  }
  let asked = false
  $effect(() => {
    if (tab === 'State' && !asked) {
      asked = true
      loadTable(data?.tables?.[0] || 'health')
    }
  })

  const mark = (b) => (b ? '✓' : '·')
  const ago = (t) => (t ? `${Math.max(0, Math.round(Date.now() / 1000 - t))}s ago` : 'never')
  const familyName = (f) => ({ ipv4: 'IPv4', ipv6: 'IPv6' })[f] || f
  const pct = (p) => (p === null || p === undefined ? '' : `${p >= 10 ? Math.round(p) : p.toFixed(1)}% free`)
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Networking' }, { label: 'Pod network (flowsdn)' }]}
    title="Pod network"
    scope={data?.node ? `flowsdn on ${data.node}` : 'flowsdn'}
    count={snap && !cilium ? endpoints.length : null}
  >
    {#snippet status()}{#if data}<StatusPill health={data.health} />{/if}{/snippet}
  </PageHeader>

  {#if error}<p class="error">{error}</p>{/if}

  {#if data && cilium}
    <EmptyState icon="network" title="Not this edition" hint={data.sentence} />
  {:else if data}
    <p class="lede">
      <span class="dim">{data.sentence}</span>
      {#if snap.answered && !snap.reachable}
        <span class="warn">Showing what it said {ago(snap.answered)}.</span>
      {:else if snap.reachable && snap.error}
        <span class="warn">Part of it did not answer: {snap.error}</span>
      {/if}
    </p>
    <p class="dim small">
      This node's agent, at <span class="mono">{snap.url}</span>. It listens on the node's loopback only, so each
      node's console shows its own.
      {#if data.hidden}<span class="warn">{data.hidden} hidden — {data.note}</span>{/if}
    </p>

    <nav class="tabs" aria-label="Pod network sections">
      {#each TABS as t}
        <button class:active={tab === t} aria-current={tab === t ? 'page' : undefined} onclick={() => (tab = t)}>{t}</button>
      {/each}
    </nav>

    {#if tab === 'Endpoints' || tab === 'Services'}
      <div class="bar">
        <input bind:value={search} placeholder="Search pods, addresses, interfaces, identities" aria-label="Search" />
        {#if tab === 'Endpoints'}
          <label>Node
            <select bind:value={node} aria-label="Node">
              <option value="">all ({nodes.length})</option>
              {#each nodes as n}<option value={n}>{n}</option>{/each}
            </select>
          </label>
        {/if}
        <label>Namespace
          <select bind:value={ns} aria-label="Namespace">
            <option value="">all</option>
            {#each namespaces as n}<option value={n}>{n}</option>{/each}
          </select>
        </label>
      </div>
    {/if}

    {#if tab === 'Endpoints'}
      {#if openError}<p class="error">{openError}</p>{/if}
      {#if open}
        {@const e = open.row}
        <section class="card wide">
          <div class="cardhead">
            <h2>Endpoint {e.id}</h2>
            <button onclick={() => (open = null)}>Close</button>
          </div>
          <div class="cols">
            <ul class="kv">
              <li><span class="k">pod</span> {#if e.namespace && e.pod}<a href="#/pod/{e.namespace}/{e.pod}">{e.namespace}/{e.pod}</a>{:else}{e.attachment || '—'}{/if}</li>
              <li><span class="k">node</span> {e.node || '—'}</li>
              <li><span class="k">state</span> {e.state}</li>
              <li><span class="k">IPv4</span> {e.ipv4.join(', ') || '—'}{#if e.ipv4.length}<CopyButton value={e.ipv4.join(' ')} label="Copy" />{/if}</li>
              <li><span class="k">IPv6</span> {e.ipv6.join(', ') || '—'}</li>
              <li><span class="k">gateways</span> {e.gateways.join(', ') || '—'}</li>
              <li><span class="k">MAC</span> {e.mac || '—'}</li>
            </ul>
            <ul class="kv">
              <li><span class="k">identity</span> {e.identity ?? 'none yet'}{#if open.identity} — {open.identity.labels?.join(' ')}{/if}</li>
              <li><span class="k">interface</span> {e.interface || '—'} → {e.container_interface || '—'}</li>
              <li><span class="k">attachment</span> <span class="mono">{e.attachment || '—'}</span></li>
              <li><span class="k">sandbox</span> <span class="mono">{e.sandbox || '—'}</span></li>
              <li><span class="k">owner</span> {e.workloads.join(', ') || '—'}</li>
              <li><span class="k">containers</span> {e.containers.join(', ') || '—'}</li>
              <li>
                <span class="k">link</span>
                {#if open.link?.error}<span class="bad">{open.link.error}</span>
                {:else}connected {mark(open.link?.connected)} · bpf {open.link?.bpf} · policy {open.link?.policy}{/if}
              </li>
            </ul>
          </div>
          {#if e.labels.length}<p class="dim small">labels: {e.labels.join(' ')}</p>{/if}
          <details><summary>What the agent said</summary><pre>{JSON.stringify(open.endpoint, null, 2)}</pre></details>
        </section>
      {/if}

      {#if !endpoints.length}
        <EmptyState icon="network" title="No endpoints" hint={snap.answered ? 'The agent has no pod attached on this node.' : 'The agent has not answered yet.'} />
      {:else}
        <div class="table-wrap">
          <table>
            <thead>
              <tr><th>Pod</th><th>Node</th><th title="state: ready">Ready</th><th>IPv4</th><th>IPv6</th><th>Identity</th><th>Interface</th><th>Owner</th></tr>
            </thead>
            <tbody>
              {#each shown as e (e.id)}
                <tr class:bad-row={!e.ready}>
                  <td>
                    <button class="link" onclick={() => show(String(e.id))}>{e.namespace && e.pod ? `${e.namespace}/${e.pod}` : e.attachment || `endpoint ${e.id}`}</button>
                  </td>
                  <td>{e.node || '—'}</td>
                  <td class="mark" title={e.state}>{mark(e.ready)}{#if !e.ready} <span class="dim">{e.state}</span>{/if}</td>
                  <td class="mono">{e.ipv4.join(', ')}</td>
                  <td class="mono">{e.ipv6.join(', ')}</td>
                  <td class="mono">{e.identity ?? '·'}</td>
                  <td class="mono">{e.interface}</td>
                  <td>{e.workloads.join(', ')}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
        {#if !shown.length}<p class="dim">Nothing matches the filters.</p>{/if}
      {/if}
    {:else if tab === 'IPAM'}
      {#if !snap.pools.length}
        <EmptyState icon="network" title="No pools" hint="The agent reports no address pool: its configuration enables no family." />
      {:else}
        <table>
          <thead><tr><th>Pool</th><th>Family</th><th>CIDR</th><th>Capacity</th><th>Allocated</th><th>Excluded</th><th>Available</th><th></th></tr></thead>
          <tbody>
            {#each snap.pools as p}
              <tr>
                <td>{p.pool}</td><td>{familyName(p.family)}</td><td class="mono">{p.cidr}</td>
                <td class="num">{p.capacity}</td><td class="num">{p.allocated}</td>
                <td class="num">{p.excluded}{#if p.allocated_excluded !== '0'} <span class="dim">({p.allocated_excluded} allocated)</span>{/if}</td>
                <td class="num">{p.available}</td>
                <td><span class:warn={p.free_pct !== null && p.free_pct < 10}>{pct(p.free_pct)}</span></td>
              </tr>
            {/each}
          </tbody>
        </table>
        <p class="dim small">Allocated counts every reservation, pending ones and the node's own included — it is not the number of endpoints.</p>
      {/if}
    {:else if tab === 'Health'}
      {#if snap.healthz}
        <ul class="kv card">
          <li><span class="k">agent</span> {snap.healthz.state} — {snap.healthz.message}</li>
          {#if snap.healthz.kubernetes}
            {@const k = snap.healthz.kubernetes}
            <li><span class="k">kubernetes</span> {k.state} — {k.message}</li>
            <li><span class="k">direct node routes</span> {mark(k.direct_routes)}</li>
            <li><span class="k">service load-balancing</span> {mark(k.service_lb)}</li>
          {:else}
            <li><span class="k">kubernetes</span> not in Kubernetes mode</li>
          {/if}
        </ul>
      {/if}
      <table>
        <thead><tr><th>Module</th><th>Level</th><th>Message</th><th>Error</th><th>Updated</th><th>Last OK</th></tr></thead>
        <tbody>
          {#each snap.modules as m}
            <tr>
              <td class="mono">{m.id}</td>
              <td><StatusPill health={m.health} label={m.level} /></td>
              <td>{m.message}</td>
              <td>{m.error}{#if m.health === 'idle'} <span class="dim">— flowsdn does not have it yet</span>{/if}</td>
              <td class="mono small">{m.updated}</td>
              <td class="mono small">{m.last_ok || 'never'}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else if tab === 'Config'}
      <p class="dim small">What the agent believes — the thing to compare with its ConfigMap.</p>
      {#if snap.config?.status}
        <ul class="kv card">
          {#each Object.entries(snap.config.status) as [k, v]}
            <li><span class="k">{k}</span> {typeof v === 'object' ? JSON.stringify(v) : v}</li>
          {/each}
        </ul>
      {/if}
      <details><summary>As the agent sent it</summary><pre>{JSON.stringify(snap.config, null, 2)}</pre></details>
    {:else if tab === 'Services'}
      {#if snap.services === null}
        <EmptyState icon="network" title="Not served" hint="The agent programs Services only in Kubernetes mode with service-lb; this one does not." />
      {:else if !services.length}
        <EmptyState icon="network" title="No Services" hint="The agent has programmed none (it lists them once its Service and EndpointSlice lists are complete)." />
      {:else}
        <table>
          <thead><tr><th>Service</th><th>Type</th><th>Frontend</th><th>Scope</th><th>Backends</th><th title="programmed in the socket-LB maps">Programmed</th></tr></thead>
          <tbody>
            {#each services as s}
              <tr>
                <td>{s.namespace}/{s.name}</td><td>{s.kind}</td><td class="mono">{s.frontend}</td><td>{s.scope}</td>
                <td class="mono small">{s.backends.join(', ') || '—'}</td>
                <td class="mark">{mark(s.realized)}{#if s.id} <span class="dim">#{s.id}</span>{/if}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    {:else if tab === 'Routes'}
      {#if snap.routes === null}
        <EmptyState icon="network" title="Not served" hint="Direct node routes are a Kubernetes-mode agent's; this one is standalone." />
      {:else if !snap.routes.length}
        <EmptyState icon="network" title="No node routes" hint="None yet: before the first Node list, with auto-direct-node-routes off, or with no other node." />
      {:else}
        <table>
          <thead><tr><th>Destination</th><th>Gateway</th><th>Node</th><th>State</th></tr></thead>
          <tbody>
            {#each snap.routes as r}
              <tr class:bad-row={r.state && r.state !== 'installed'}>
                <td class="mono">{r.destination}</td><td class="mono">{r.gateway}</td><td>{r.node}</td><td>{r.state}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    {:else if tab === 'State'}
      <div class="bar">
        {#each data.tables as t}<button onclick={() => loadTable(t)}>Table: {t}</button>{/each}
        <span class="dim small">The agent's StateDB, one named table at a time. It serves only the health table today.</span>
      </div>
      {#if tableError}<p class="error">{tableError}</p>{/if}
      {#if table}
        <table>
          <thead><tr><th>Rev</th><th>ID</th><th>Level</th><th>Message</th><th>Error</th><th>Count</th></tr></thead>
          <tbody>
            {#each table.rows as r}
              <tr><td class="num">{r.rev}</td><td class="mono">{r.row.id}</td><td>{r.row.level}</td><td>{r.row.message}</td><td>{r.row.error}</td><td class="num">{r.row.count}</td></tr>
            {/each}
          </tbody>
        </table>
      {/if}
    {:else if tab === 'Flows'}
      <EmptyState icon="network" title="No flows yet" hint={data.flows} />
    {/if}
  {/if}
</div>

<style>
  .lede { margin: 0 0 4px; }
  .small { font-size: var(--sc-t-meta); }
  .dim { color: var(--text-dim); }
  .warn { color: var(--warn-strong); }
  .error, .bad { color: var(--error); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .tabs { display: flex; gap: 2px; border-bottom: 1px solid var(--border); margin: 12px 0 14px; flex-wrap: wrap; }
  .tabs button { background: none; border: 0; border-bottom: 2px solid transparent; border-radius: 0; padding: 6px 12px; color: var(--text-dim); cursor: pointer; }
  .tabs button:hover { color: var(--text); background: var(--nav-hover); }
  .tabs button.active { color: var(--text); border-bottom-color: var(--accent); font-weight: 600; }
  .bar { display: flex; gap: 12px; align-items: center; flex-wrap: wrap; margin-bottom: 12px; }
  .bar > input { width: 320px; max-width: 100%; }
  .bar label { display: inline-flex; gap: 6px; align-items: center; font-size: var(--sc-t-meta); }
  .table-wrap { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); margin-bottom: 12px; font-size: var(--sc-t-body); }
  th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); padding: 8px 12px; border-bottom: 1px solid var(--border); }
  td { padding: 6px 12px; vertical-align: top; }
  tr + tr > td { border-top: 1px solid var(--sc-hairline); }
  tbody tr { cursor: default; }
  tr.bad-row td:first-child { box-shadow: inset 3px 0 var(--warn-strong); }
  td.mark { font-weight: 700; }
  td.num { font-variant-numeric: tabular-nums; font-family: var(--mono); font-size: var(--sc-t-meta); word-break: break-all; }
  button.link { background: none; border: 0; padding: 0; color: var(--accent); cursor: pointer; font: inherit; text-align: left; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px var(--sc-row-px); margin-bottom: 12px; }
  .cardhead { display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px; }
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0; }
  .cols { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 12px; }
  .kv { list-style: none; margin: 0; padding: 0; display: grid; gap: 3px; font-size: var(--sc-t-meta); }
  ul.kv.card { padding: 12px var(--sc-row-px); }
  .kv .k { color: var(--text-faint); font-family: var(--mono); margin-right: 6px; }
  details { margin-top: 8px; font-size: var(--sc-t-meta); }
  pre { max-height: 360px; overflow: auto; background: var(--bg); padding: 8px; border-radius: var(--radius-sm); font-size: 12px; }
</style>
