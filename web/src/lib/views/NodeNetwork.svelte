<script>
  // A node's Network page (#336, stormcos#959): every host port from its NetworkPort object
  // (stormpump#236) — settings, state, module and counters — and its charts from ironprom.
  //
  // A setting is changed as a patch of the object's `spec`, as you, through the apiserver. The node's
  // PID 1 applies it live and stormport writes the outcome into `status`, which this page reads again
  // every few seconds, so the change is seen coming back. Only FEC is applied by the node today; the
  // other settings are accepted and said "not applied yet" (stormpump#246).
  import DataGrid from 'stormview/components/DataGrid.svelte'
  import { route } from '../router.svelte.js'
  import { feed } from '../stores.svelte.js'
  import { get, postJson, formatBytes } from '../api.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import Icon from '../components/Icon.svelte'
  import EntityCharts from '../components/EntityCharts.svelte'
  import { levelHref } from '../charts.js'
  import {
    portRows, wantWord, moduleLines, appliedWords, counterRows, draftOf, changeOf, changeLine, lastChangeLine,
  } from '../ports.js'

  const POLL_MS = 3000

  const host = $derived(route.current.params.host)
  const picked = $derived(route.current.query.get('port') || '')

  let data = $state(null)
  let error = $state('')
  // Each port's last two different counter readings, for rates. Kept in load(), not in an effect.
  let readings = $state({})
  let draft = $state(null)
  let editing = $state(false)
  let saying = $state('')
  let saved = $state('')
  let busy = $state(false)
  let timer = null

  // ironprom holds the series of the node it runs on, so a port's charts are on its own node's console.
  const node = $derived(feed.components.find((c) => c.kind === 'node' && (c.label === host || c.id === `fleet:node:${host}`)))
  const isLocal = $derived(node?.id === 'fleet:node:local')

  async function load() {
    try {
      const d = await get(`/api/plugins/k8s/nodes/${encodeURIComponent(host)}/ports`)
      const at = Date.now()
      const next = {}
      for (const p of d.ports || []) {
        const was = readings[p.name]
        const same = was && JSON.stringify(was.cur.counters) === JSON.stringify(p.counters)
        next[p.name] = same ? was : { prev: was?.cur || null, cur: { at, counters: p.counters } }
      }
      readings = next
      data = d
      error = ''
    } catch (e) {
      error = e.message
    }
    clearTimeout(timer)
    timer = setTimeout(load, POLL_MS)
  }
  $effect(() => {
    if (host) load()
    return () => clearTimeout(timer)
  })

  const port = $derived((data?.ports || []).find((p) => p.name === picked) || null)
  const rows = $derived(portRows(data?.ports))

  // Rates over the last two readings of this port that differ.
  const counters = $derived.by(() => {
    if (!port) return []
    const r = readings[port.name]
    const secs = r?.prev ? (r.cur.at - r.prev.at) / 1000 : 0
    return counterRows(port.counters, r?.prev?.counters, secs)
  })

  function pick(name) {
    editing = false
    saying = ''
    saved = ''
    location.hash = `#/node/${encodeURIComponent(host)}/network${name ? `?port=${encodeURIComponent(name)}` : ''}`
  }

  function edit() {
    draft = draftOf(port.spec)
    saying = ''
    saved = ''
    editing = true
  }

  const change = $derived(editing && draft && port ? changeOf(draft, port.spec) : {})
  const changeKeys = $derived(Object.keys(change).filter((k) => k !== 'error'))
  const notToday = (k) => !(data?.appliedToday || []).includes(k)

  async function save() {
    if (change.error || !changeKeys.length) return
    busy = true
    try {
      const r = await postJson(`/api/plugins/k8s/ports/${encodeURIComponent(port.name)}`, change, 'PATCH')
      saved = r.message
      saying = ''
      editing = false
      await load()
    } catch (e) {
      saying = e.message
    }
    busy = false
  }

  const columns = [
    { key: 'health', label: '', render: 'health', width: '28px' },
    { key: 'ifname', label: 'Interface', render: 'mono' },
    { key: 'link', label: 'Link' },
    { key: 'speed', label: 'Speed', render: (r) => r.speedText },
    { key: 'fec', label: 'FEC', render: (r) => r.fecText },
    { key: 'want', label: 'FEC held to' },
    { key: 'module', label: 'Module' },
    { key: 'applied', label: 'Spec' },
    { key: 'mac', label: 'MAC', render: 'mono' },
  ]

  const rate = (c) => {
    if (c.rate === null) return ''
    const v = c.key.endsWith('Bytes') ? `${formatBytes(c.rate)}/s` : `${c.rate >= 10 ? Math.round(c.rate) : c.rate.toFixed(1)}/s`
    return v
  }
  const applied = $derived(port ? appliedWords(port.applied, port.pending) : null)
  const settingLabel = { fec: 'FEC', mtu: 'MTU', speed: 'Speed (Mb/s)', autoneg: 'Autoneg', pause: 'Pause', rings: 'Rings', offloads: 'Offloads' }
  const shown = (v) => (v === null || v === undefined ? '—' : typeof v === 'object' ? JSON.stringify(v) : String(v))
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Compute' }, { label: 'Nodes', href: '#/nodes' }, { label: host, href: `#/node/${encodeURIComponent(host)}` }, { label: 'Network' }]}
    title={port ? `${host} ${port.ifname}` : `${host}: network ports`}
    count={data && !port ? data.ports.length : null}
  >
    {#snippet actions()}
      {#if port}
        <button onclick={() => pick('')}><Icon name="table" size={13} /> All ports</button>
      {/if}
      <a class="sc-btn" href={levelHref('network', host)}>Network charts</a>
    {/snippet}
  </PageHeader>
  <p class="lead">
    Every physical port on this node, from its <code>NetworkPort</code> object: what stormpump reads from the card,
    and what is asked of it. A change goes as you, as a patch of the object's <code>spec</code>; the node applies it
    live, with no reboot, and this page shows its answer coming back.
  </p>

  {#if error && !data}
    <EmptyState icon="node" title="The ports cannot be read" hint="The kubernetes plugin returned: {error}" />
  {:else if !data}
    <div class="sc-empty"><p>Reading {host}'s ports…</p></div>
  {:else if !data.ports.length}
    <EmptyState icon="node" title="No ports to show" hint={data.note || 'No NetworkPort names this node.'} />
  {:else if !port}
    {#if error}<p class="bad small" role="alert">Last read failed: {error}</p>{/if}
    <DataGrid {columns} {rows} onrowclick={(r) => pick(r.id)} />
    <p class="dim small">Live: read again every {POLL_MS / 1000} s. Pick a port for its settings, module, counters and charts.</p>
  {:else}
    {#if error}<p class="bad small" role="alert">Last read failed: {error}</p>{/if}
    <section class="cards" data-port={port.name}>
      <div class="card">
        <h2>State</h2>
        <dl>
          <dt>Link</dt><dd data-link class={port.state === 'up' ? 'ok' : 'warn'}>{port.state}</dd>
          <dt>Speed</dt><dd>{port.speedWord}{port.supportedSpeed ? ` of ${port.supportedSpeed / 1000}G` : ''}</dd>
          <dt>FEC in use</dt><dd class="mono" data-fec-active>{port.fec.active || '—'}</dd>
          <dt>FEC configured</dt><dd class="mono">{port.fec.configured || '—'}</dd>
          <dt>FEC held to</dt><dd>{wantWord(port.fec)}</dd>
          <dt>Pause</dt><dd>rx {port.pause?.rx === true ? 'on' : port.pause?.rx === false ? 'off' : '—'}, tx {port.pause?.tx === true ? 'on' : port.pause?.tx === false ? 'off' : '—'}</dd>
          <dt>MAC</dt><dd class="mono">{port.mac || '—'}</dd>
          <dt>Driver</dt><dd class="mono">{[port.driver?.name, port.driver?.version].filter(Boolean).join(' ') || '—'}</dd>
          <dt>Firmware</dt><dd class="mono">{port.driver?.firmware || '—'}</dd>
          <dt>Bus</dt><dd class="mono">{port.driver?.bus || '—'}</dd>
          <dt>Last change</dt><dd data-last-change>{lastChangeLine(port.lastChange) || '—'}</dd>
        </dl>
      </div>
      <div class="card">
        <h2>Module</h2>
        {#if moduleLines(port.module).length}
          <dl>
            {#each moduleLines(port.module) as [label, value] (label)}<dt>{label}</dt><dd>{value}</dd>{/each}
          </dl>
        {:else}
          <p class="dim small">{port.module?.present === false ? 'The cage is empty.' : 'No module: a BASE-T port has none, and no FEC.'}</p>
        {/if}
      </div>
      <div class="card">
        <h2>Settings</h2>
        <p class="applied {applied.tone}" data-applied>Spec: {applied.word}{applied.message ? ` — ${applied.message}` : ''}</p>
        {#if editing}
          <div class="form">
            <label>FEC
              <select bind:value={draft.fec} aria-label="FEC">
                <option value="">left to the node's setting</option>
                {#each data.fecModes as m (m)}<option value={m}>{m}</option>{/each}
              </select>
            </label>
            <label>MTU <input bind:value={draft.mtu} aria-label="MTU" placeholder="as it is" inputmode="numeric" /></label>
            <label>Speed (Mb/s) <input bind:value={draft.speed} aria-label="Speed" placeholder="as it is" inputmode="numeric" /></label>
            <label>Autoneg
              <select bind:value={draft.autoneg} aria-label="Autoneg"><option value="">as it is</option><option value="on">on</option><option value="off">off</option></select>
            </label>
            <label>Pause rx
              <select bind:value={draft.pauseRx} aria-label="Pause rx"><option value="">as it is</option><option value="on">on</option><option value="off">off</option></select>
            </label>
            <label>Pause tx
              <select bind:value={draft.pauseTx} aria-label="Pause tx"><option value="">as it is</option><option value="on">on</option><option value="off">off</option></select>
            </label>
            <label>Ring rx <input bind:value={draft.ringRx} aria-label="Ring rx" placeholder="as it is" inputmode="numeric" /></label>
            <label>Ring tx <input bind:value={draft.ringTx} aria-label="Ring tx" placeholder="as it is" inputmode="numeric" /></label>
          </div>
          {#if change.error}
            <p class="bad small" role="alert">{change.error}</p>
          {:else if changeKeys.length}
            <p class="small" data-change>Will ask for: {changeLine(change)}</p>
            {#each changeKeys.filter(notToday) as k (k)}
              <p class="warn small">{settingLabel[k] || k}: the node does not apply this yet ({data.notAppliedIssue}); it is recorded and said "not applied yet".</p>
            {/each}
          {/if}
          <button class="sc-primary" disabled={busy || !!change.error || !changeKeys.length} onclick={save}>{busy ? 'Saving…' : 'Save'}</button>
          <button disabled={busy} onclick={() => (editing = false)}>Cancel</button>
        {:else}
          <dl>
            {#each Object.keys(settingLabel) as k (k)}
              <dt>{settingLabel[k]}</dt>
              <dd class="mono" data-setting={k}>{k in port.spec ? shown(port.spec[k]) : 'as the port has it'}</dd>
            {/each}
          </dl>
          {#if data.may}
            <button class="sc-primary" onclick={edit}>Change settings</button>
          {:else}
            <p class="dim small" data-why>{data.why}</p>
          {/if}
        {/if}
        {#if saved}<p class="ok small" data-saved>{saved}</p>{/if}
        {#if saying}<p class="bad small" role="alert" data-refused>{saying}</p>{/if}
      </div>
    </section>

    <section class="card">
      <h2>Counters</h2>
      {#if counters.length}
        <table class="counters">
          <thead><tr><th>Counter</th><th>Total</th><th>Rate</th></tr></thead>
          <tbody>
            {#each counters as c (c.key)}
              <tr class:bad={c.bad} data-counter={c.key}><td>{c.label}</td><td class="mono">{c.value.toLocaleString()}</td><td class="mono">{rate(c)}</td></tr>
            {/each}
          </tbody>
        </table>
        <p class="dim small">Rates are since the last read. Per-queue counters are not served yet.</p>
      {:else}
        <p class="dim small">The node has reported no counters for this port.</p>
      {/if}
    </section>

    <section class="card charts">
      <h2>Over time</h2>
      {#if isLocal}
        <EntityCharts query={{ entity: 'port', node: host, ifname: port.ifname }} />
      {:else}
        <p class="dim small">This port's history is kept by {host}'s own ironprom: open this page on that node's console for its charts.</p>
      {/if}
    </section>
  {/if}
</div>

<style>
  .lead { margin: 0 0 12px; color: var(--text-dim); font-size: var(--sc-t-body); max-width: 70em; }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 12px;
    align-items: start;
    margin-bottom: 12px;
  }
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px var(--sc-row-px);
    margin-bottom: 12px;
  }
  .cards .card { margin-bottom: 0; }
  h2 {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin: 0 0 10px;
  }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 4px 14px; margin: 0 0 10px; }
  dt { font-size: var(--sc-t-meta); color: var(--text-faint); }
  dd { margin: 0; font-size: var(--sc-t-body); overflow-wrap: anywhere; }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
  .ok { color: var(--ok); }
  .warn { color: var(--warn-strong); }
  .bad { color: var(--error); }
  .applied { margin: 0 0 10px; font-size: var(--sc-t-body); }
  .applied.dim { color: var(--text-dim); }
  .form { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 8px 12px; margin-bottom: 10px; }
  .form label { display: flex; flex-direction: column; gap: 3px; font-size: var(--sc-t-meta); color: var(--text-faint); }
  .counters { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  .counters th { text-align: left; font-size: var(--sc-t-meta); color: var(--text-faint); font-weight: 500; padding: 4px 8px; border-bottom: 1px solid var(--border); }
  .counters td { padding: 3px 8px; border-bottom: 1px solid var(--border); }
  .counters tr.bad td { color: var(--error); }
</style>
