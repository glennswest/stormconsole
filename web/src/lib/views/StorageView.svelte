<script>
  // The storage.storm.io objects (#96): what the engine and stormdrive say of themselves. Read-only; the actions
  // stay on the pages that have their guards (#82).
  import { onDestroy } from 'svelte'
  import { get, formatBytes } from '../api.js'
  import { route, navigate } from '../router.svelte.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import { volumeRows, slabRows, nodeRows, driveRows, enclosureRows, totals, tone, okOf, matches } from '../storage.js'

  const TABS = ['volumes', 'slabs', 'drives', 'nodes', 'enclosures']
  const tab = TABS.includes(route.current.query.get('tab')) ? route.current.query.get('tab') : 'volumes'
  const SHOWN = 300

  let sb = $state({})
  let phy = $state({})
  let search = $state('')
  let error = $state('')
  let timer = null

  const read = (path) => get(path).catch((e) => ({ items: [], error: e.message }))
  async function load() {
    const [volumes, slabs, drives, nodes, physical, enclosures] = await Promise.all([
      read('/api/plugins/sb/resources/volumes'),
      read('/api/plugins/sb/resources/slabs'),
      read('/api/plugins/sb/resources/drives'),
      read('/api/plugins/sb/resources/nodes'),
      read('/api/plugins/drive/resources/drives'),
      read('/api/plugins/drive/resources/enclosures'),
    ])
    sb = { volumes, slabs, drives, nodes }
    phy = { drives: physical, enclosures }
    error = volumes.error || ''
    clearTimeout(timer)
    timer = setTimeout(load, 10000)
  }
  load()
  onDestroy(() => clearTimeout(timer))

  const volumes = $derived(volumeRows(sb.volumes?.items))
  const slabs = $derived(slabRows(sb.slabs?.items))
  const nodes = $derived(nodeRows(sb.nodes?.items))
  const drives = $derived(driveRows(sb.drives?.items, phy.drives?.items))
  const enclosures = $derived(enclosureRows(phy.enclosures?.items))
  const t = $derived(totals({ volumes, slabs, nodes }))
  const silent = $derived([...(phy.drives?.nodes || [])].filter((n) => !n.ok))

  const counts = $derived({ volumes: volumes.length, slabs: slabs.length, drives: drives.length, nodes: nodes.length, enclosures: enclosures.length })
  const rows = $derived({ volumes, slabs, drives, nodes, enclosures }[tab].filter((r) => matches(r, search)))
  const shown = $derived(rows.slice(0, SHOWN))
  const bytes = (v) => (v || v === 0 ? formatBytes(v) : '—')
  const dash = (v) => (v === null || v === undefined || v === '' ? '—' : v)
  const words = { volumes: 'volumes', slabs: 'slabs', drives: 'drives', nodes: 'nodes', enclosures: 'enclosures' }
</script>

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Storage' }, { label: 'Overview' }]} title="Storage" scope="this node's engine, every node's drives" />

  {#if error}<p class="bad">{error}</p>{/if}
  {#each silent as n (n.node)}<p class="warn">{n.node}’s stormdrive did not answer: {n.error}</p>{/each}

  <section class="band">
    <div><span class="k">Capacity</span> <strong>{bytes(t.capacityBytes)}</strong></div>
    <div><span class="k">Free</span> <strong>{bytes(t.freeBytes)}</strong></div>
    <div><span class="k">Volumes</span> <strong>{t.volumes}</strong>{#if t.trouble}<span class="warn gap">{t.trouble} need attention</span>{/if}</div>
    <div><span class="k">Slabs</span> <strong>{t.slabs}</strong>{#if t.quarantined}<span class="warn gap">{t.quarantined} quarantined</span>{/if}</div>
    <div><span class="k">Drives</span> <strong>{drives.length}</strong></div>
    <div><span class="k">Enclosures</span> <strong>{enclosures.length}</strong></div>
  </section>

  <nav class="tabs" aria-label="Storage objects">
    {#each TABS as x (x)}
      <button class:active={tab === x} onclick={() => navigate(`#/storage?tab=${x}`)}>{x[0].toUpperCase() + x.slice(1)} <span class="n">{counts[x]}</span></button>
    {/each}
    <input class="search" bind:value={search} placeholder="Search" aria-label={`Search ${words[tab]}`} />
  </nav>

  {#if !rows.length}
    <EmptyState icon="storage" title={search ? `No ${words[tab]} match` : `No ${words[tab]}`}
      hint={search ? '' : tab === 'enclosures' ? 'An internal shelf behind a controller is read when stormdrive can read it. Drives lists what it has found.' : sb[tab]?.error || phy[tab]?.error || ''} />
  {:else}
    {#if tab === 'volumes'}
      <table>
        <thead><tr><th>Name</th><th>Health</th><th>Legs</th><th>Size</th><th>Allocated</th><th>On disk</th><th>Redundancy</th><th>Role</th><th>Retention</th><th>Attached</th></tr></thead>
        <tbody>
          {#each shown as v (v.id)}
            <tr class:trouble={v.trouble}>
              <td><a href={`#/grid?id=${encodeURIComponent(`sb:volume:${v.id}`)}`}>{v.name}</a>{#if v.sealed}<span class="dim gap">sealed</span>{/if}</td>
              <td class={tone(v.health)}>{dash(v.health)}{#if v.unreadable}<span class="bad gap">unreadable</span>{/if}{#if v.failedSlabs}<span class="bad gap">{v.failedSlabs} slab(s) failed</span>{/if}</td>
              <td class:warn={v.legsMissing}>{v.legsExpected ? `${v.legsExpected - v.legsMissing} of ${v.legsExpected}` : '—'}</td>
              <td>{bytes(v.sizeBytes)}</td><td>{bytes(v.allocatedBytes)}</td><td>{bytes(v.physicalBytes)}</td>
              <td>{dash(v.redundancy)}</td><td>{dash(v.role)}</td><td>{dash(v.retention)}</td><td>{dash(v.attach)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else if tab === 'slabs'}
      <table>
        <thead><tr><th>Drive</th><th>Serial</th><th>Tier</th><th>Role</th><th>Capacity</th><th>Free</th><th>Domain</th><th>State</th></tr></thead>
        <tbody>
          {#each shown as s (s.id)}
            <tr class:trouble={s.quarantined}>
              <td class="mono">{dash(s.path)}</td><td class="mono dim">{dash(s.serial)}</td><td>{dash(s.tier)}</td><td>{dash(s.role)}</td>
              <td>{bytes(s.capacityBytes)}</td><td>{bytes(s.freeBytes)}</td><td class="mono dim">{dash(s.domain)}</td>
              <td>{#if s.quarantined}<span class="bad">quarantined</span>{:else}in use{/if}{#if s.metadata}<span class="dim gap">holds metadata</span>{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else if tab === 'drives'}
      <table>
        <thead><tr><th>Node</th><th>Drive</th><th>Serial</th><th>Model</th><th>Capacity</th><th>Bay</th><th>Health</th><th>Temp</th><th>Wear</th><th>Slabs</th><th>In the fleet</th></tr></thead>
        <tbody>
          {#each shown as d (d.key)}
            <tr>
              <td>{d.node || 'this node'}</td><td class="mono">{dash(d.path)}{#if d.system}<span class="dim gap">system</span>{/if}</td>
              <td class="mono dim">{dash(d.serial)}</td><td class="dim">{dash(d.model)}</td><td>{bytes(d.capacityBytes)}</td>
              <td>{d.bay == null ? '—' : `${d.enclosure ? `${d.enclosure} ` : ''}${d.bay}`}</td>
              <td class={tone(d.health)}>{dash(d.health)}</td><td>{d.temperatureC == null ? '—' : `${d.temperatureC} °C`}</td><td>{d.wearPct == null ? '—' : `${d.wearPct} %`}</td>
              <td>{d.slabs == null ? '—' : d.slabs}{#if d.drain}<span class="warn gap">draining</span>{/if}</td>
              <td>{d.seenBy === 'engine' ? 'the engine only' : dash(d.membership)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      <p class="dim small">The engine’s and stormdrive’s views, joined by serial. The engine here is this node’s, so another node’s drive has no slab count. <a href="#/drives">The map, and what can be done to a drive</a></p>
    {:else if tab === 'nodes'}
      <table>
        <thead><tr><th>Node</th><th>Capacity</th><th>Free</th><th>Topology</th><th>Engine</th></tr></thead>
        <tbody>
          {#each shown as n (n.name)}
            <tr><td>{n.name}{#if n.local}<span class="dim gap">this node</span>{/if}</td><td>{bytes(n.totalBytes)}</td><td>{bytes(n.freeBytes)}</td><td class="dim">{dash(n.topology)}</td><td class="mono dim">{dash(n.version)}</td></tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <table>
        <thead><tr><th>Node</th><th>Enclosure</th><th>Drives</th><th>Worst drive</th><th>Status</th><th>Power</th><th>Fans</th><th>Hottest</th><th>Paths</th><th>Controllers</th></tr></thead>
        <tbody>
          {#each shown as e (`${e.node}/${e.name}`)}
            <tr>
              <td>{e.node}</td><td class="mono">{e.name}</td><td>{dash(e.driveCount)}{#if e.needsReformat}<span class="warn gap">{e.needsReformat} need a format</span>{/if}</td>
              <td class={tone(e.worstHealth)}>{dash(e.worstHealth)}</td><td class={tone(e.status)}>{e.status || 'not read'}</td>
              <td>{okOf(e.powerSupplies)}</td><td>{okOf(e.fans)}</td><td>{e.maxTemperatureC == null ? '—' : `${e.maxTemperatureC} °C`}</td><td>{dash(e.paths)}</td>
              <td class="dim">{(e.hbas || []).join(', ') || '—'}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
    {#if rows.length > SHOWN}<p class="dim small">Showing {SHOWN} of {rows.length}: search to narrow them.</p>{/if}
  {/if}
</div>

<style>
  .band { display: flex; flex-wrap: wrap; gap: 8px 28px; background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 10px var(--sc-row-px); margin-bottom: 12px; }
  .k { color: var(--text-faint); font-size: var(--sc-t-meta); margin-right: 4px; }
  .tabs { display: flex; flex-wrap: wrap; align-items: center; gap: 2px; border-bottom: 1px solid var(--border); margin-bottom: 12px; }
  .tabs button { background: none; border: none; border-bottom: 2px solid transparent; color: var(--text-dim); padding: 6px 12px; cursor: pointer; border-radius: 0; }
  .tabs button:hover { color: var(--text); background: var(--nav-hover); }
  .tabs button.active { color: var(--text); border-bottom-color: var(--accent); font-weight: 600; }
  .n { color: var(--text-faint); font-size: var(--sc-t-meta); margin-left: 2px; }
  .search { margin-left: auto; margin-bottom: 4px; width: 200px; }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  th { text-align: left; color: var(--text-faint); font-size: var(--sc-t-meta); font-weight: 600; padding: 6px 12px 6px 0; border-bottom: 1px solid var(--border); white-space: nowrap; }
  td { padding: 6px 12px 6px 0; border-bottom: 1px solid var(--sc-hairline); vertical-align: top; }
  tr.trouble td:first-child { box-shadow: inset 2px 0 0 var(--warning, var(--error)); padding-left: 6px; }
  .mono { font-family: var(--mono, monospace); }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
  .gap { margin-left: 6px; font-size: var(--sc-t-meta); }
  .warn { color: var(--warning, var(--error)); }
  .bad { color: var(--error); }
</style>
