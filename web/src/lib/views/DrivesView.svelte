<script>
  // Drives, arranged the way the hardware is (#8), at rack scale (#32).
  //
  // A drive is a physical object: it lives in a chassis, in a bay, and
  // somebody eventually walks up and pulls it. At 160 drives a node and
  // ~1,600 a rack a list of them is not a view of anything, so the default
  // is a map: each chassis drawn bay by bay, every bay coloured by the
  // question being asked — health, temperature, wear, usage — grouped by
  // chassis, node or rack, filtered to the drives that need someone, and
  // totalled in the units a rack is measured in. Click a bay for the drive,
  // its actions and what the engine holds on it.
  //
  // Each drive's usage (#29) comes from its own node's stormdrive, in bytes,
  // fetched while this page is open — used, free, outside any slab, what
  // overcommit lets the slabs promise, each slab, any drain — and the
  // volumes on it from that node's stormdrive (#52), with who uses each. Pools are
  // the same slabs summed per node, role and tier (`?group=pool`).
  //
  // The model is drivemap.js, tested at 1,600 drives without a browser.
  import { route } from '../router.svelte.js'
  import { feed } from '../stores.svelte.js'
  import { call, get } from '../api.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import StatusPill from '../components/StatusPill.svelte'
  import ResourceTable from '../components/ResourceTable.svelte'
  import Icon from '../components/Icon.svelte'
  import OfferedDrives from '../components/OfferedDrives.svelte'
  import DriveActions from '../components/DriveActions.svelte'
  import FormatDialog from '../components/FormatDialog.svelte'
  import FormatProgress from '../components/FormatProgress.svelte'
  import ReplaceDialog from '../components/ReplaceDialog.svelte'
  import FirmwareDialog from '../components/FirmwareDialog.svelte'
  import MaintainProgress from '../components/MaintainProgress.svelte'
  import ControllerBanner from '../components/ControllerBanner.svelte'
  import { replaceWhy, rows as replaceRows } from '../drivereplace.js'
  import { rows as firmwareRows } from '../drivefirmware.js'
  import { commonActions } from '../driveactions.js'
  import { progressRows, selectable as formatable } from '../driveformat.js'
  import { ago } from '../ui/time.js'
  import { build, groups as groupBy, totals, heat, cells, columns, formatBytes, FILTERS, MODES, pools as poolsOf, noUsage, setName, shelfKey, enclosures, unreadShelves, unreadShelf } from '../drivemap.js'

  const asShelves = $derived(route.current.query.get('group') === 'shelf')
  const asPools = $derived(route.current.query.get('group') === 'pool')

  // The page's own fetches (#29): not in the feed, because only this page
  // wants 1,600 drives' slab lists. Re-read every 10 s while open; the
  // plugins reuse an answer for 10–15 s, so tabs do not multiply the load.
  let usage = $state(null)
  let placement = $state(null)
  let enginePool = $state(null)
  let usageError = $state('')
  // The format records (#308): who may be formatted, and each run's progress.
  let formats = $state(null)
  let guard = $state(null)
  // Replace and firmware (#317): the firmware records, this node's RAID sets, and who may update.
  let firmware = $state(null)
  let raid = $state(null)
  let fwGuard = $state(null)
  let now = $state(Math.floor(Date.now() / 1000))
  async function refresh() {
    const [u, p, pool, f] = await Promise.allSettled([
      get('/api/plugins/drive/usage'),
      get('/api/plugins/sb/placement'),
      get('/api/plugins/sb/proxy/api/v1/slabs/pool'),
      get('/api/plugins/drive/formats'),
    ])
    if (f.status === 'fulfilled') formats = f.value
    await refreshMaintain()
    if (u.status === 'fulfilled') {
      usage = u.value
      usageError = ''
    } else usageError = String(u.reason?.message || u.reason)
    placement = p.status === 'fulfilled' ? p.value : null
    enginePool = pool.status === 'fulfilled' ? pool.value : null
  }
  async function refreshMaintain() {
    const [fw, rd] = await Promise.allSettled([get('/api/plugins/drive/firmware'), get('/api/plugins/sb/raid')])
    if (fw.status === 'fulfilled') firmware = fw.value
    raid = rd.status === 'fulfilled' ? rd.value : null
  }
  $effect(() => {
    refresh()
    get(`/api/v1/console/guard?method=POST&path=${encodeURIComponent('/api/plugins/drive/format')}`)
      .then((g) => (guard = g.guard))
      .catch((e) => (guard = { allowed: false, reason: e.message }))
    get(`/api/v1/console/guard?method=POST&path=${encodeURIComponent('/api/plugins/drive/firmware')}`)
      .then((g) => (fwGuard = g.guard))
      .catch((e) => (fwGuard = { allowed: false, reason: e.message }))
    const t = setInterval(refresh, 10000)
    const tick = setInterval(() => (now = Math.floor(Date.now() / 1000)), 1000)
    return () => {
      clearInterval(t)
      clearInterval(tick)
    }
  })
  const silent = $derived((usage?.nodes || []).filter((n) => !n.ok))

  function stored(key, fallback) {
    try {
      return localStorage.getItem(`stormconsole-drives-${key}`) || fallback
    } catch {
      return fallback
    }
  }
  function keep(key, v) {
    try {
      localStorage.setItem(`stormconsole-drives-${key}`, v)
    } catch {}
  }

  let search = $state('')
  let filter = $state('')
  let by = $state(stored('by', 'chassis'))
  let mode = $state(stored('mode', 'health'))
  let view = $state(stored('view', 'map'))
  let selected = $state(null)
  // The offered drive being asked about (stormblock#154).
  let asking = $state(null)
  let busy = $state('')
  let showAll = $state({})
  $effect(() => keep('by', by))
  $effect(() => keep('mode', mode))
  $effect(() => keep('view', view))

  const invoke = (a) => call(a.method, a.path)

  // One drive's actions, or a selection's (#308): grouped in one menu, never a row of buttons.
  let chosen = $state({})
  let selecting = $state(false)
  let dialog = $state(null)
  let actNote = $state(null)
  const formatRecs = $derived(formats?.drives || [])
  const recOf = (id) => formatRecs.find((f) => f.component === id) || null
  const fkey = (f) => `${f.node}/${f.id}`
  const progress = $derived(progressRows(formatRecs, now, 3600))
  function toggle(id) {
    const c = { ...chosen }
    if (c[id]) delete c[id]
    else c[id] = true
    chosen = c
  }
  function chooseAll(drives) {
    const c = { ...chosen }
    const on = drives.every((d) => c[d.id])
    for (const d of drives) on ? delete c[d.id] : (c[d.id] = true)
    chosen = c
  }
  /// Why this drive cannot be formatted from here, or '' when it can.
  function formatWhy(d) {
    if (guard && !guard.allowed) return guard.reason || 'you may not format drives'
    const f = recOf(d.id)
    if (!f) return formats ? 'its node’s stormdrive has no format record for it' : 'reading the format records…'
    if (!formatable(f)) return f.whyNot || 'a format is running on it'
    return ''
  }
  function openFormat(ids) {
    dialog = { picked: ids.map(recOf).filter(Boolean).map(fkey) }
  }

  // Replace… and Update firmware… (#317).
  let replacing = $state(null)
  let fwDialog = $state(null)
  const fwRecs = $derived(firmware?.drives || [])
  const fwOf = (id) => fwRecs.find((f) => f.component === id) || null
  const ctx = $derived({ formats: new Map(formatRecs.map((f) => [f.component, f])), raid, components: feed.components })
  function firmwareWhy(d) {
    if (fwGuard && !fwGuard.allowed) return fwGuard.reason || 'you may not update firmware'
    const f = fwOf(d.id)
    if (!f) return firmware ? 'its node’s stormdrive has no firmware record for it' : 'reading the firmware records…'
    return f.whyNot || ''
  }
  function openFirmware(ids) {
    const recs = ids.map(fwOf).filter(Boolean)
    if (recs.length) fwDialog = recs
  }
  // What this page asked to be replaced, kept in the browser so a row can say what was asked; each
  // row's state is read again from the engine and stormdrive.
  const DAY = 86400
  function loadAsks() {
    try {
      const all = JSON.parse(localStorage.getItem('stormconsole-drives-replaces') || '[]')
      return all.filter((a) => Date.now() / 1000 - a.at < DAY)
    } catch {
      return []
    }
  }
  let asks = $state(loadAsks())
  function keepAsks(list) {
    asks = list
    try {
      localStorage.setItem('stormconsole-drives-replaces', JSON.stringify(list))
    } catch {}
  }
  const replaceProgress = $derived(replaceRows(asks, { records: model.records, raid }))
  const firmwareProgress = $derived(firmwareRows(fwRecs, now, 3600))
  async function runOne(a, label) {
    if (a.danger && !confirm(`${a.label} ${label}?`)) return
    try {
      const r = await invoke(a)
      actNote = { ok: true, text: (r && (r.message || r.detail)) || `${a.label}: ${label}` }
    } catch (e) {
      if (e.message !== 'cancelled') actNote = { ok: false, text: `${a.label} ${label}: ${e.message}` }
    }
  }
  async function runMany(a) {
    const on = a.on || []
    if (a.danger && !confirm(`${a.label} on ${on.length} ${on.length === 1 ? 'drive' : 'drives'}?`)) return
    let failed = 0
    for (const { drive, action } of on) {
      try {
        await invoke(action)
      } catch (e) {
        failed += 1
        console.error(drive.id, e)
      }
    }
    actNote = { ok: !failed, text: `${a.label}: ${on.length - failed} of ${on.length} done${failed ? `, ${failed} failed` : ''}` }
  }

  const model = $derived(build(feed.components, { usage, placement }))
  const poolRows = $derived(poolsOf(model.records))
  const pct = (a, b) => (b ? `${Math.round((a / b) * 100)}%` : '—')
  /// Where a component id leads, by the rule ResourceTable uses.
  function hrefOf(id) {
    const c = id && model.byId.get(id)
    return c?.link || (id ? `#/grid?id=${encodeURIComponent(id)}` : null)
  }
  const records = $derived(model.records)
  const shelves = $derived(model.shelves)
  const all = $derived(totals(records))
  const shown = $derived(groupBy(records, { by, filter, search }))
  const shownCount = $derived(shown.reduce((n, g) => n + g.drives.length, 0))
  const shelfCount = $derived(new Set(records.map(shelfKey)).size)
  const boxCount = $derived(enclosures(records))
  const unread = $derived(unreadShelves(records))
  const pick = $derived(selected ? records.find((d) => d.id === selected) : null)
  const recById = $derived(new Map(records.map((r) => [r.id, r])))
  const picks = $derived(records.filter((d) => chosen[d.id]))
  const pickActions = $derived(commonActions(picks.map((d) => d.c)))
  // The drive list with each drive's use as a metric, so the list says it too.
  const shownComponents = $derived(
    feed.components.map((c) => {
      const d = c.kind === 'drive' ? recById.get(c.id) : null
      if (!d) return c
      let metrics = c.metrics || []
      // A drive the RAID controller holds says so first, never as missing (#325).
      if (d.controllerFailed) metrics = [{ label: 'controller', value: 'Failed by the controller', tone: 'error' }, ...metrics.filter((m) => m.label !== 'controller')]
      if (d.use?.in_use) metrics = [{ label: 'use', value: d.use.badge || 'In use', tone: d.use.use === 'system' ? 'error' : 'warn' }, ...metrics]
      return metrics === c.metrics ? c : { ...c, metrics }
    }),
  )
  // Within a node or a rack, still one grid per shelf: the bays only mean
  // something inside their shelf.
  const sub = (g) => (by === 'chassis' ? [g] : groupBy(g.drives, { by: 'chassis' }))

  async function shelfAction(shelf, a) {
    if (a.danger && !confirm(`${a.label} ${shelf.label}?`)) return
    busy = a.id
    try {
      await invoke(a)
    } catch (e) {
      console.error(e)
    }
    busy = ''
  }

  const LIST_CAP = 200
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Hardware' }, { label: asShelves ? 'Shelves' : asPools ? 'Pools' : 'Drives' }]}
    title={asShelves ? 'Shelves' : asPools ? 'Pools' : 'Drives'}
    count={feed.loaded ? (asShelves ? shelves.length + unread.length : asPools ? poolRows.length : records.length) : null}
  >
    {#snippet actions()}
      {#if !asShelves && !asPools}
        <a class="pagebtn plain" href="#/drives/controllers" data-testid="controllers-page">Controllers</a>
        <a class="pagebtn" href="#/drives/format" data-testid="format-page">Format drives…</a>
      {/if}
    {/snippet}
    {#snippet status()}
      <span class="fleet" title="Drives enrolled in the storage fleet">
        {records.filter((d) => !d.outOfFleet).length.toLocaleString()}/{records.length.toLocaleString()} in fleet
      </span>
    {/snippet}
  </PageHeader>

  <!-- A controller in RAID mode is why its drives look missing (#325): said on every Drives page. -->
  <ControllerBanner />

  {#if feed.loaded && !asShelves && !asPools}
    <!-- Outside the drive list: an engine can hold a drive no stormdrive reports. -->
    <OfferedDrives bind:asking />
  {/if}

  {#if !feed.loaded}
    <div class="sc-empty"><p>Connecting to the component feed…</p></div>
  {:else if asShelves}
    {#each unread as u (u.key)}
      <!-- Drives on a backplane are on a shelf, read or not (#166). -->
      <p class="unread" data-testid="unread-shelf">
        {u.node}: {u.drives} {u.drives === 1 ? 'drive' : 'drives'} in the {u.words}. <a href="#/drives">See the drives</a>
      </p>
    {/each}
    {#if shelves.length === 0 && unread.length === 0}
      <EmptyState icon="storage" title="No shelves" hint="No stormdrive in this cluster reports a drive." />
    {:else if shelves.length}
      <!-- Expanding a shelf shows its drives, through the has_many edge
           stormdrive already publishes. -->
      <ResourceTable
        components={feed.components}
        rootIds={shelves.map((s) => s.id)}
        {invoke}
        showKind={false}
      />
    {/if}
  {:else if asPools}
    <!-- Pools (#29): every slab on every drive, per node, role and tier.
         From stormdrive's usage, so every node that answers is here. -->
    {#if enginePool}
      <p class="dim pressure">
        This node's engine: {Math.round(enginePool.used_pct ?? 0)}% of its slabs used{enginePool.enabled ? ` · growth at ${enginePool.high_water_pct}%` : ' · automatic growth off'}{#if enginePool.under_pressure}<span class="warn"> · under pressure</span>{/if}
      </p>
    {/if}
    {#if poolRows.length === 0}
      <EmptyState
        icon="storage"
        title="No pools reported"
        hint={usageError
          ? `The drive usage could not be read: ${usageError}`
          : usage
            ? 'No node’s stormdrive reports slabs on its drives. Usage needs stormdrive v0.13.0 or later, and an engine whose slab listing it can read.'
            : 'Reading each node’s drives…'}
      />
    {:else}
      <table class="pools">
        <thead>
          <tr><th>Node</th><th>Role</th><th>Tier</th><th class="n">Drives</th><th class="n">Slabs</th><th>Written</th><th class="n">Total</th><th class="n">Free</th><th class="n">May promise</th><th class="n">Committed</th><th class="n">Headroom</th></tr>
        </thead>
        <tbody>
          {#each poolRows as p (p.node + p.role + p.tier)}
            <tr>
              <td>{p.node}</td><td>{p.role}</td><td>{p.tier}</td>
              <td class="n">{p.drives}</td><td class="n">{p.slabs}</td>
              <td><div class="usebar small" title="{pct(p.used, p.total)} written"><span class="used" class:hot={p.frac >= 0.9} style="width: {p.frac * 100}%"></span></div></td>
              <td class="n">{formatBytes(p.total)}</td>
              <td class="n" class:warn={p.frac >= 0.9}>{formatBytes(p.free)}</td>
              <td class="n" title={p.promisable > p.total ? 'overcommitted: the slabs may promise more than they hold' : ''}>{formatBytes(p.promisable)}{p.promisable > p.total ? ' ↑' : ''}</td>
              <td class="n">{p.committed === null ? '—' : formatBytes(p.committed)}</td>
              <td class="n" class:warn={p.headroom !== null && p.headroom < p.promisable / 10}>{p.headroom === null ? 'not reported' : formatBytes(p.headroom)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      <p class="dim">“Committed” is what volumes have been promised out of these slabs; it is shown only where every slab reports it (stormblock#152), because a partial sum reads as headroom that is not there. “May promise” is the slabs’ size times each drive’s overcommit ratio.</p>
    {/if}
    {#if silent.length}
      <p class="warn">Not read: {silent.map((n) => `${n.node} (${n.error})`).join(', ')}</p>
    {/if}
  {:else if records.length === 0}
    <EmptyState
      icon="storage"
      title="No drives discovered"
      hint="No node's stormdrive has reported a disk. Either none is running, or these machines have nothing it can see — check the Hardware card on the overview."
    >
      {#snippet action()}
        <a class="sc-back" href="#/">Back to overview</a>
      {/snippet}
    </EmptyState>
  {:else}
    <!-- The rack in one line, each number a filter. -->
    <div class="band">
      <span class="big">{all.drives.toLocaleString()} <small>drives</small></span>
      <span class="big">{all.nodes} <small>node{all.nodes === 1 ? '' : 's'}</small></span>
      <span class="big">{boxCount} <small>{boxCount === 1 ? 'enclosure' : 'enclosures'}</small></span>
      <span class="big">{shelfCount} <small>{shelfCount === 1 ? 'shelf' : 'shelves'}</small></span>
      <span class="big">{formatBytes(all.capacity)} <small>raw</small></span>
      {#if all.withUsage}
        <span class="big" title="Of the {all.withUsage} drives whose usage is known">{formatBytes(all.used)} <small>used · {formatBytes(all.left)} left of {formatBytes(all.usageCapacity)}</small></span>
      {/if}
      <span class="chips">
        {#each [['failing', all.failing, 'error'], ['degraded', all.degraded, 'warn'], ['rebuilding', all.rebuilding, 'warn'], ['draining', all.draining, 'warn'], ['full', all.full, 'warn'], ['hot', all.hot, 'warn']] as [f, n, tone]}
          <button class="chip {n ? tone : ''}" class:on={filter === f} disabled={!n && filter !== f} onclick={() => (filter = filter === f ? '' : f)}>
            {n} {f}
          </button>
        {/each}
      </span>
    </div>

    <div class="bar">
      <input bind:value={search} placeholder="Search: serial, model, bay, node, device" aria-label="Search drives" />
      <select bind:value={filter} aria-label="Filter">
        {#each FILTERS as [v, l]}<option value={v}>{l}</option>{/each}
      </select>
      <label>group by
        <select bind:value={by} aria-label="Group by">
          <option value="chassis">shelf</option>
          <option value="node">node</option>
          <option value="rack">rack</option>
          <option value="set">RAID set</option>
        </select>
      </label>
      <label>colour by
        <select bind:value={mode} aria-label="Colour by" disabled={view !== 'map'}>
          {#each MODES as [v, l]}<option value={v}>{l}</option>{/each}
        </select>
      </label>
      <span class="seg" role="group" aria-label="View">
        <button class:sel={view === 'map'} onclick={() => (view = 'map')}>Map</button>
        <button class:sel={view === 'list'} onclick={() => (view = 'list')}>List</button>
      </span>
      <button class:sel={selecting} aria-pressed={selecting} onclick={() => (selecting = !selecting)} data-testid="select-mode">{selecting ? 'Selecting: click drives' : 'Select drives'}</button>
      <span class="hint">{shownCount === records.length ? `${shown.length} ${by === 'chassis' ? (shown.length === 1 ? 'shelf' : 'shelves') : by === 'set' ? 'RAID sets' : `${by}s`}` : `${shownCount.toLocaleString()} of ${records.length.toLocaleString()}`}</span>
    </div>

    {#if view === 'map'}
      <div class="legend">
        {#if mode === 'health'}
          <span><i style="background: var(--ok)"></i>healthy</span>
          <span><i style="background: var(--warn-strong)"></i>warning</span>
          <span><i style="background: var(--error)"></i>failing</span>
          <span><i style="background: var(--text-faint)"></i>unknown</span>
        {:else}
          <span>{mode === 'temp' ? '25 °C' : '0%'}</span>
          <span class="ramp"></span>
          <span>{mode === 'temp' ? '60 °C' : '100%'}</span>
          <span><i class="nodata"></i>not reported</span>
          {#if mode === 'usage' && silent.length}<span class="warn">no usage from {silent.map((n) => n.node).join(', ')}</span>{/if}
        {/if}
        <span><i class="ring"></i>rebuilding</span>
        <span><i class="inuse-key"></i>in use (system, slab, RAID)</span>
        {#if selecting}<span><i class="chosen-key"></i>selected</span>{/if}
      </div>
    {/if}

    {#if actNote}
      <p class="actnote" class:err={!actNote.ok} role="status">{actNote.text} <button class="link" onclick={() => (actNote = null)}>dismiss</button></p>
    {/if}

    {#if picks.length}
      <div class="selbar" data-testid="selection">
        <strong>{picks.length} selected</strong>
        <span class="dim">{picks.filter((d) => d.use?.in_use).length} in use</span>
        <button class="danger" disabled={!picks.some((d) => !formatWhy(d))} onclick={() => openFormat(picks.map((d) => d.id))} data-testid="format-selected">Format…</button>
        <button disabled={!picks.some((d) => !firmwareWhy(d))} title={picks.map((d) => firmwareWhy(d)).find(Boolean) || ''} onclick={() => openFirmware(picks.filter((d) => !firmwareWhy(d)).map((d) => d.id))} data-testid="firmware-selected">Update firmware…</button>
        <DriveActions actions={pickActions} run={runMany} count={picks.length} label="Actions for {picks.length}" />
        <button class="link" onclick={() => (chosen = {})}>Clear</button>
      </div>
    {/if}

    {#if progress.length}
      <section class="picked" data-testid="drives-progress">
        <h3 class="first">Formats</h3>
        <FormatProgress rows={progress} {now} />
      </section>
    {/if}

    {#if replaceProgress.length || firmwareProgress.length}
      <section class="picked" data-testid="maintain-progress">
        <h3 class="first">Replacements and firmware</h3>
        <MaintainProgress replaces={replaceProgress} updates={firmwareProgress} ondismiss={(r) => keepAsks(asks.filter((a) => !(a.old.id === r.old.id && a.at === r.at)))} />
      </section>
    {/if}

    {#if replacing}
      <ReplaceDialog
        old={replacing}
        records={model.records}
        {ctx}
        onclose={() => (replacing = null)}
        onformat={(d) => { replacing = null; openFormat([d.id]) }}
        ondone={(a) => { replacing = null; keepAsks([a, ...asks]); refresh() }}
      />
    {/if}

    {#if fwDialog}
      <FirmwareDialog
        recs={fwDialog}
        images={firmware?.images || []}
        guard={fwGuard}
        onreload={async () => { await refreshMaintain(); fwDialog = fwDialog.map((r) => fwOf(r.component) || r) }}
        onclose={() => (fwDialog = null)}
        ondone={() => { fwDialog = null; chosen = {}; refresh() }}
      />
    {/if}

    {#if dialog}
      <FormatDialog records={formatRecs} picked={dialog.picked} {guard} onclose={() => (dialog = null)} ondone={() => { dialog = null; chosen = {}; refresh() }} />
    {/if}

    {#if pick}
      <section class="picked">
        <header>
          <strong>{pick.c.label}</strong>
          <span class="dim">{pick.node}{pick.shelfLabel ? ` · ${pick.shelfLabel}` : ` · ${unreadShelf(pick)}`}{pick.bay !== null ? ` · bay ${pick.bay}` : ''}{pick.hba ? ` · controller ${pick.hba}` : ''}{pick.serial ? ` · ${pick.serial}` : ''}</span>
          {#if pick.use?.in_use}<span class="badge" class:system={pick.use.use === 'system'} data-testid="picked-use">{pick.use.badge || 'In use'}</span>{/if}
          {#if pick.controllerFailed}<span class="badge system" data-testid="picked-controller">Failed by the controller</span>{/if}
          <span class="tools">
            {#if pick.serial && !pick.c.id.startsWith('drive:@')}<a class="metrics" href={`#/metrics?entity=drive&serial=${encodeURIComponent(pick.serial)}`}>Metrics</a>{/if}
            {#if !formatWhy(pick)}<button class="danger" onclick={() => openFormat([pick.id])} data-testid="picked-format">Format…</button>{/if}
            <DriveActions
              actions={pick.c.actions || []}
              run={(a) => runOne(a, pick.c.label)}
              onformat={() => openFormat([pick.id])}
              formatWhy={formatWhy(pick)}
              onreplace={() => (replacing = pick)}
              replaceWhy={replaceWhy(pick)}
              onfirmware={() => openFirmware([pick.id])}
              firmwareWhy={firmwareWhy(pick)}
            />
            <button onclick={() => (selected = null)}>Close</button>
          </span>
        </header>
        {#if pick.controllerFailed}
          <p class="useline" data-testid="picked-controller-line">
            <strong>Failed by the controller:</strong> its RAID controller{pick.hba ? ` (${pick.hba})` : ''} holds it as {pick.controller}, so Linux does not see it. It is not missing: Set good (in Actions, under Controller) brings it back. <a href="#/drives/controllers">Controllers</a>
          </p>
        {/if}
        {#if pick.use?.in_use}
          <div class="useline" data-testid="picked-reason">
            <strong>In use:</strong> {pick.use.reason}.
            {#if pick.use.depends_on?.length}<span class="dim">Depends on it: {pick.use.depends_on.join(', ')}.</span>{/if}
            {#if pick.use.slabs?.length}<span class="dim">Slabs: {pick.use.slabs.map((x) => `${x.name || 'slab'}${x.partition ? ` (partition ${x.partition})` : ''}, ${x.role}`).join('; ')}.</span>{/if}
            <span class="dim">Not offered for format.</span>
          </div>
        {:else if pick.use?.holds}
          <p class="dim">Not in use. It holds {pick.use.holds}, which a format destroys.</p>
        {/if}
        {#if pick.offer}
          <p class="offerline">
            {#if pick.offer.offered}
              <span class="warn">Offered: {pick.offer.sentence}.</span> Nothing formats it on its own.
              {#if pick.offer.action}<button class="danger" onclick={() => (asking = pick.offer)}>Format as a slab…</button>{/if}
            {:else}
              <span class="dim">{pick.offer.sentence}.</span>
            {/if}
          </p>
        {/if}
        {#if pick.usage}
          {@const u = pick.usage}
          {@const cap = pick.capacity || u.slab + u.unslabbed}
          <div class="usebar" title="used / free in slabs / not in a slab">
            <span class="used" style="width: {(u.used / cap) * 100}%"></span>
            <span class="free" style="width: {(u.free / cap) * 100}%"></span>
          </div>
          <p class="dim"><strong>{formatBytes(u.left)} left</strong> of {formatBytes(cap)} · {formatBytes(u.used)} used · {formatBytes(u.free)} free in slabs · {formatBytes(u.unslabbed)} not in a slab{u.source === 'engine' ? ' — from this node’s engine; its stormdrive does not report usage (v0.13.0)' : ''}</p>
          {#if pick.overcommit || u.committed !== null}
            <p class="dim">
              Overcommit {pick.overcommit?.enabled ? `${pick.overcommit.ratio}×` : 'off'}{#if u.promisable} · may promise {formatBytes(u.promisable)}{/if}{#if u.committed !== null} · committed {formatBytes(u.committed)} · <span class:warn={u.headroom < u.promisable / 10}>headroom {formatBytes(u.headroom)}</span>{:else} · committed not reported (stormblock#152){/if}
            </p>
          {/if}
        {:else}
          <p class="dim">{noUsage(pick)}.</p>
        {/if}
        {#if pick.drain}
          {@const dr = pick.drain}
          {@const all_ = dr.moved + dr.remaining + dr.failed}
          <div class="drain">
            <span class:warn={dr.state === 'running' || dr.state === 'stuck'}>Drain {dr.state}</span>
            {#if all_}<div class="usebar small"><span class="used" style="width: {(dr.moved / all_) * 100}%"></span></div>{/if}
            <span class="dim">{dr.moved} moved · {dr.remaining} left{dr.failed ? ` · ${dr.failed} failed` : ''}{dr.reason ? ` · asked by ${dr.reason}` : ''}{dr.then_leave ? ' · leaves the fleet when empty' : ''}</span>
            {#each dr.errors || [] as e}<span class="error">{e}</span>{/each}
          </div>
        {/if}
        {#if setName(pick)}
          <!-- The set's name in the room (#43): rack label · the engine's shelf and set. -->
          <p class:warn={pick.member !== 'active'}>RAID set <span class="mono">{setName(pick)}</span> · {pick.member}</p>
        {:else if pick.member}<p class="warn">Array member: {pick.member}</p>{/if}
        {#if pick.usage?.slabs?.length}
          <h3>Slabs on this drive</h3>
          <table class="pools">
            <thead><tr><th>Slab</th><th>Role</th><th>Tier</th><th>Written</th><th class="n">Used</th><th class="n">Free</th><th class="n">Size</th><th class="n">Committed</th></tr></thead>
            <tbody>
              {#each pick.usage.slabs as sl (sl.id)}
                {@const vs = (pick.volumes || []).flatMap((v) => v.slabs).find((x) => x.id === sl.id)}
                <tr>
                  <td class="mono">{sl.id}{#if vs && vs.state !== 'ok'}{' '}<span class="warn">{vs.state}</span>{/if}</td>
                  <td>{sl.role}</td><td>{sl.tier}</td>
                  <td><div class="usebar small"><span class="used" class:hot={sl.total && sl.used / sl.total >= 0.9} style="width: {sl.total ? (sl.used / sl.total) * 100 : 0}%"></span></div></td>
                  <td class="n">{formatBytes(sl.used)}</td><td class="n">{formatBytes(sl.free)}</td><td class="n">{formatBytes(sl.total)}</td>
                  <td class="n">{sl.committed === null || sl.committed === undefined ? '—' : formatBytes(sl.committed)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
        <h3>Volumes on this drive</h3>
        {#if pick.volumes?.length}
          <table class="pools">
            <thead><tr><th>Volume</th><th>Kind</th><th>Used by</th><th class="n">Here</th><th class="n">Legs</th><th>State</th></tr></thead>
            <tbody>
              {#each pick.volumes as v (v.id)}
                <tr>
                  <td>{#if v.component}<a href={hrefOf(v.component)}>{v.name}</a>{:else}{v.name}{/if}</td>
                  <td>{v.kind}</td>
                  <td>{#if v.consumer}{#if v.consumer_link}<a href={hrefOf(v.consumer_link)}>{v.consumer}</a>{:else}{v.consumer}{/if}{:else}<span class="dim">nothing</span>{/if}</td>
                  <td class="n">{formatBytes(v.bytes)}</td>
                  <td class="n" title={v.shared_legs ? `${v.shared_legs} shared with another volume (an instance and the registry image it was cloned from)` : ''}>{v.legs}{v.shared_legs ? ` (${v.shared_legs} shared)` : ''}</td>
                  <td class:warn={v.state !== 'ok'}>{v.state}{v.rebuild && v.rebuild !== 'none' ? ` · rebuild ${v.rebuild}` : ''}{v.policy ? ` · ${v.policy}` : ''}</td>
                </tr>
              {/each}
            </tbody>
          </table>
          {#if pick.volumesAt && Date.now() / 1000 - pick.volumesAt > 300}
            <p class="dim small">As {pick.node}’s stormdrive last heard from its engine, {ago(new Date(pick.volumesAt * 1000).toISOString())} ago.</p>
          {/if}
        {:else}
          <p class="dim">
            {#if pick.volumesReported}
              No volume has data on this drive.
            {:else if !placement}
              This node’s engine did not answer for placement{pick.local ? '' : ', and it is the only engine this console reads'}.
            {:else if placement.error}
              The engine’s placement could not be read: {placement.error}.
            {:else if placement.volumes && !placement.placed}
              This engine does not report where volumes live (placement, stormblock v17.1.0).
            {:else if !pick.local}
              No volume of this node’s engine is on it, and {pick.node}’s stormdrive does not report its own: its engine may not report placement, or it may be older than stormdrive#26.
            {:else}
              No volume has data on this drive.
            {/if}
          </p>
        {/if}
        <ResourceTable components={shownComponents} rootIds={[pick.id]} {invoke} showKind={false} />
      </section>
    {/if}

    {#if shownCount === 0}
      <EmptyState icon="filter" title="No matches" hint="No drive matches the current search and filter." />
    {:else}
      {#each shown as g (g.key)}
        <section class="group">
          <header>
            <span class="mark"><Icon name="storage" size={16} /></span>
            <h2>{g.label}</h2>
            <span class="detail">
              {g.totals.drives} drives · {formatBytes(g.totals.capacity)}
              {#if g.totals.failing}<span class="error"> · {g.totals.failing} failing</span>{/if}
              {#if g.totals.degraded}<span class="warn"> · {g.totals.degraded} degraded</span>{/if}
              {#if g.totals.rebuilding}<span class="warn"> · {g.totals.rebuilding} rebuilding</span>{/if}
            </span>
            {#if selecting}
              <button class="selgroup" onclick={() => chooseAll(g.drives)}>{g.drives.every((d) => chosen[d.id]) ? 'Unselect' : 'Select'} {by === 'chassis' ? 'shelf' : by}</button>
            {/if}
            {#if by === 'chassis'}
              {@const shelf = model.byId.get(g.key)}
              {#if shelf}
                <StatusPill health={shelf.health} />
                <span class="acts">
                  {#each (shelf.metrics || []).filter((m) => ['psu', 'fans', 'temp'].includes(m.label)) as m}
                    <span class="m"><span class="ml">{m.label}</span><span class="mv {m.tone || ''}">{m.value}{m.unit || ''}</span></span>
                  {/each}
                  {#each shelf.actions || [] as a}
                    <button class:danger={a.danger} disabled={!a.enabled || busy === a.id} onclick={() => shelfAction(shelf, a)}>{a.label}</button>
                  {/each}
                </span>
              {/if}
            {/if}
          </header>

          {#if view === 'map'}
            <div class="chassis-row">
              {#each sub(g) as ch (ch.key)}
                <div class="chassis">
                  {#if by !== 'chassis'}<div class="clabel">{ch.label}</div>{/if}
                  <div class="bays" style="grid-template-columns: repeat({columns(ch.drives)}, 1fr)">
                    {#each cells(ch.drives) as d, i}
                      {@const h = heat(d, mode)}
                      {#if d}
                        <button
                          class="bay"
                          class:nodata={h.css === null}
                          class:rebuild={d.member === 'rebuilding' || d.member === 'degraded'}
                          class:failing={d.health === 'error' || d.controllerFailed}
                          class:sel={selected === d.id}
                          class:inuse={d.use?.in_use}
                          class:chosen={!!chosen[d.id]}
                          style={h.css ? `background: ${h.css}` : ''}
                          title="{d.bay !== null ? `bay ${d.bay} · ` : ''}{d.c.label} · {h.text}{d.health !== 'ok' ? ` · ${d.health}` : ''}{d.member ? ` · ${d.member}` : ''}{d.use?.in_use ? ` · in use: ${d.use.badge || 'in use'}` : ''}{d.controllerFailed && mode !== 'health' ? ' · Failed by the controller' : ''}"
                          aria-label="{d.c.label}, {h.text}{d.use?.in_use ? `, in use: ${d.use.badge || 'in use'}` : ''}"
                          aria-pressed={selecting ? !!chosen[d.id] : undefined}
                          data-drive={d.c.label}
                          onclick={() => (selecting ? toggle(d.id) : (selected = selected === d.id ? null : d.id))}
                        ></button>
                      {:else}
                        <span class="bay empty" title="bay {i}: empty"></span>
                      {/if}
                    {/each}
                  </div>
                </div>
              {/each}
            </div>
          {:else}
            {@const ids = g.drives.map((d) => d.id)}
            <ResourceTable
              components={shownComponents}
              rootIds={showAll[g.key] ? ids : ids.slice(0, LIST_CAP)}
              {invoke}
              showKind={false}
            />
            {#if ids.length > LIST_CAP && !showAll[g.key]}
              <button class="more" onclick={() => (showAll[g.key] = true)}>Show all {ids.length}</button>
            {/if}
          {/if}
        </section>
      {/each}
    {/if}
  {/if}
</div>

<style>
  .fleet {
    font-size: var(--sc-t-meta);
    font-family: var(--mono);
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  .group + .group { margin-top: 18px; }
  .group header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 2px 8px 0;
  }
  .mark { color: var(--text-ghost); display: grid; place-items: center; }
  h2 { font-size: 14px; font-weight: 600; margin: 0; white-space: nowrap; }
  .detail { font-size: var(--sc-t-meta); color: var(--text-dim); }
  .acts { margin-left: auto; display: flex; align-items: center; gap: 10px; white-space: nowrap; }
  .acts button { font-size: var(--sc-t-eyebrow); padding: 3px 9px; }

  .m { display: inline-flex; gap: 5px; align-items: baseline; }
  .ml {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-faint);
  }
  .mv { font-family: var(--mono); font-size: var(--sc-t-meta); font-weight: 600; }
  .mv.warn { color: var(--warn-strong); }
  .mv.error { color: var(--error); }
  .mv.muted { color: var(--text-dim); font-weight: 400; }
  .mv.accent { color: var(--accent); }

  .bare {
    margin: 0;
    padding: 12px var(--sc-row-px);
    font-size: var(--sc-t-body);
    color: var(--text-dim);
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .sc-back { font-size: var(--sc-t-body); }

  .band { display: flex; flex-wrap: wrap; gap: 18px; align-items: baseline; margin-bottom: 12px; }
  .big { font-size: 20px; font-weight: 600; font-variant-numeric: tabular-nums; }
  .big small { font-size: var(--sc-t-meta); font-weight: 400; color: var(--text-dim); }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; margin-left: auto; }
  .chip { font-size: var(--sc-t-meta); padding: 2px 9px; border-radius: 999px; }
  .chip.error { color: var(--error); border-color: var(--error); }
  .chip.warn { color: var(--warn-strong); border-color: var(--warn-strong); }
  .chip.on { background: var(--nav-hover); font-weight: 600; }
  .bar { display: flex; flex-wrap: wrap; gap: 10px; align-items: center; margin-bottom: 10px; }
  .bar input { width: 300px; max-width: 100%; }
  .bar label { display: inline-flex; gap: 6px; align-items: center; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .seg { display: inline-flex; }
  .seg button { border-radius: 0; }
  .seg button.sel { background: var(--nav-hover); font-weight: 600; }
  .hint { font-size: var(--sc-t-meta); color: var(--text-dim); margin-left: auto; }
  .legend { display: flex; flex-wrap: wrap; gap: 12px; align-items: center; font-size: var(--sc-t-meta); color: var(--text-dim); margin-bottom: 10px; }
  .legend span { display: inline-flex; gap: 5px; align-items: center; }
  .legend i { width: 12px; height: 12px; border-radius: 2px; display: inline-block; }
  .legend .ramp { width: 120px; height: 10px; border-radius: 2px; background: linear-gradient(90deg, hsl(130 65% 45%), hsl(65 65% 45%), hsl(0 65% 45%)); }
  .legend i.nodata, .bay.nodata { background: repeating-linear-gradient(45deg, var(--panel), var(--panel) 3px, var(--border) 3px, var(--border) 5px); }
  .legend i.ring { border: 2px solid var(--accent); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .warn { color: var(--warn-strong); }
  .error { color: var(--error); }
  .unread { margin: 0 0 12px; padding: 7px 12px; font-size: var(--sc-t-meta); color: var(--text-dim); background: var(--panel); border: 1px solid var(--border); border-left: 3px solid var(--accent); border-radius: var(--radius-sm); }
  .chassis-row { display: flex; flex-wrap: wrap; gap: 12px; }
  .chassis { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 8px; min-width: 200px; }
  .clabel { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); margin-bottom: 6px; }
  .bays { display: grid; gap: 3px; }
  .bay { width: 18px; height: 18px; padding: 0; border: 1px solid transparent; border-radius: 3px; cursor: pointer; }
  .bay.empty { background: none; border: 1px dashed var(--border); cursor: default; }
  .bay.failing { border-color: var(--error); }
  .bay.rebuild { outline: 2px solid var(--accent); outline-offset: 0; }
  .bay.sel { outline: 2px solid var(--text); outline-offset: 1px; }
  .bay:focus-visible { outline: 2px solid var(--text); }
  .picked { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 10px var(--sc-row-px); margin-bottom: 14px; }
  .picked header { display: flex; flex-wrap: wrap; gap: 8px 10px; align-items: center; margin-bottom: 8px; }
  .picked header .metrics { font-size: var(--sc-t-meta); }
  .usebar { display: flex; height: 10px; background: var(--border); border-radius: 3px; overflow: hidden; }
  .usebar .used { background: var(--accent); }
  .usebar .free { background: var(--ok); opacity: 0.5; }
  .more { margin-top: 6px; font-size: var(--sc-t-meta); }
  .usebar.small { height: 6px; min-width: 80px; }
  .usebar .used.hot { background: var(--warn-strong); }
  .picked h3 { font-size: var(--sc-t-meta); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); margin: 12px 0 4px; font-weight: 600; }
  table.pools { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); font-variant-numeric: tabular-nums; }
  table.pools th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); font-weight: 600; padding: 4px 8px; border-bottom: 1px solid var(--border); }
  table.pools td { padding: 4px 8px; border-bottom: 1px solid var(--border); }
  table.pools .n { text-align: right; }
  table.pools td.warn { color: var(--warn-strong); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .offerline { margin: 0 0 8px; }
  .offerline button.danger { margin-left: 8px; color: var(--error); }
  .drain { display: flex; flex-wrap: wrap; gap: 10px; align-items: center; margin: 6px 0; font-size: var(--sc-t-meta); }
  .pressure { margin-bottom: 10px; }
  .pagebtn { font-size: var(--sc-t-meta); padding: 4px 11px; border: 1px solid var(--error-border, var(--error)); border-radius: var(--radius-sm); color: var(--error); text-decoration: none; white-space: nowrap; }
  .pagebtn.plain { border-color: var(--border-strong); color: var(--text); }
  .badge { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; padding: 1px 7px; border-radius: 999px; border: 1px solid var(--warn-strong); color: var(--warn-strong); white-space: nowrap; }
  .badge.system { border-color: var(--error); color: var(--error); }
  .tools { margin-left: auto; display: flex; flex-wrap: wrap; gap: 8px; align-items: center; justify-content: flex-end; }
  .tools button { font-size: var(--sc-t-meta); }
  button.danger { color: var(--error); }
  .useline { margin: 0 0 8px; font-size: var(--sc-t-body); display: grid; gap: 2px; }
  .selbar { display: flex; flex-wrap: wrap; gap: 10px; align-items: center; padding: 8px 12px; margin-bottom: 10px; background: var(--accent-bg); border: 1px solid var(--border-strong); border-radius: var(--radius); font-size: var(--sc-t-body); }
  .selbar button { font-size: var(--sc-t-meta); }
  .selgroup { font-size: var(--sc-t-eyebrow); padding: 2px 8px; }
  .bar button.sel { background: var(--nav-hover); font-weight: 600; }
  .actnote { margin: 0 0 10px; padding: 6px 12px; font-size: var(--sc-t-meta); border-radius: var(--radius-sm); background: var(--ok-bg); color: var(--ok); border: 1px solid var(--ok-border); }
  .actnote.err { background: var(--error-bg); color: var(--error); border-color: var(--error-border); }
  button.link { background: none; border: none; padding: 0; color: var(--accent); font-size: inherit; }
  .picked h3.first { margin-top: 0; }
  /* In use: a corner mark on the bay, whatever it is coloured by. */
  .bay { position: relative; }
  .bay.inuse::after, .legend i.inuse-key::after { content: ''; position: absolute; right: 1px; top: 1px; width: 6px; height: 6px; border-radius: 50%; background: var(--text); box-shadow: 0 0 0 1px var(--panel); }
  .legend i.inuse-key { position: relative; background: var(--text-faint); }
  .bay.chosen, .legend i.chosen-key { box-shadow: inset 0 0 0 2px var(--accent); }
  .legend i.chosen-key { border: 2px solid var(--accent); }
</style>
