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
  // volumes on it from this node's engine, with who uses each. Pools are
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
  import { build, groups as groupBy, totals, heat, cells, columns, formatBytes, FILTERS, MODES, pools as poolsOf, noUsage } from '../drivemap.js'

  const asShelves = $derived(route.current.query.get('group') === 'shelf')
  const asPools = $derived(route.current.query.get('group') === 'pool')

  // The page's own fetches (#29): not in the feed, because only this page
  // wants 1,600 drives' slab lists. Re-read every 10 s while open; the
  // plugins reuse an answer for 10–15 s, so tabs do not multiply the load.
  let usage = $state(null)
  let placement = $state(null)
  let enginePool = $state(null)
  let usageError = $state('')
  async function refresh() {
    const [u, p, pool] = await Promise.allSettled([
      get('/api/plugins/drive/usage'),
      get('/api/plugins/sb/placement'),
      get('/api/plugins/sb/proxy/api/v1/slabs/pool'),
    ])
    if (u.status === 'fulfilled') {
      usage = u.value
      usageError = ''
    } else usageError = String(u.reason?.message || u.reason)
    placement = p.status === 'fulfilled' ? p.value : null
    enginePool = pool.status === 'fulfilled' ? pool.value : null
  }
  $effect(() => {
    refresh()
    const t = setInterval(refresh, 10000)
    return () => clearInterval(t)
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
  let busy = $state('')
  let showAll = $state({})
  $effect(() => keep('by', by))
  $effect(() => keep('mode', mode))
  $effect(() => keep('view', view))

  const invoke = (a) => call(a.method, a.path)

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
  const chassisCount = $derived(new Set(records.map((d) => d.shelfId || `${d.node}/`)).size)
  const pick = $derived(selected ? records.find((d) => d.id === selected) : null)
  // Within a node or a rack, still one grid per chassis: the bays only mean
  // something inside their enclosure.
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
    count={feed.loaded ? (asShelves ? shelves.length : asPools ? poolRows.length : records.length) : null}
  >
    {#snippet status()}
      <span class="fleet" title="Drives enrolled in the storage fleet">
        {records.filter((d) => !d.outOfFleet).length.toLocaleString()}/{records.length.toLocaleString()} in fleet
      </span>
    {/snippet}
  </PageHeader>

  {#if !feed.loaded}
    <div class="sc-empty"><p>Connecting to the component feed…</p></div>
  {:else if asShelves}
    {#if shelves.length === 0}
      <EmptyState
        icon="storage"
        title="No shelves"
        hint="stormdrive reports no enclosure. Drives attached directly to a controller have no shelf — they are on the Drives page."
      >
        {#snippet action()}
          <a class="sc-back" href="#/drives">See the drives</a>
        {/snippet}
      </EmptyState>
    {:else}
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
      <span class="big">{chassisCount} <small>chassis</small></span>
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
          <option value="chassis">chassis</option>
          <option value="node">node</option>
          <option value="rack">rack</option>
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
      <span class="hint">{shownCount === records.length ? `${shown.length} ${by === 'chassis' ? 'chassis' : `${by}s`}` : `${shownCount.toLocaleString()} of ${records.length.toLocaleString()}`}</span>
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
      </div>
    {/if}

    {#if pick}
      <section class="picked">
        <header>
          <strong>{pick.c.label}</strong>
          <span class="dim">{pick.node}{pick.shelfLabel ? ` · ${pick.shelfLabel}` : ''}{pick.bay !== null ? ` · bay ${pick.bay}` : ''}{pick.hba ? ` · controller ${pick.hba}` : ''}{pick.serial ? ` · ${pick.serial}` : ''}</span>
          <button onclick={() => (selected = null)}>Close</button>
        </header>
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
        {#if pick.member}<p class="warn">Array member: {pick.member}</p>{/if}
        {#if pick.usage?.slabs?.length}
          <h3>Slabs on this drive</h3>
          <table class="pools">
            <thead><tr><th>Slab</th><th>Role</th><th>Tier</th><th>Written</th><th class="n">Used</th><th class="n">Free</th><th class="n">Size</th><th class="n">Committed</th></tr></thead>
            <tbody>
              {#each pick.usage.slabs as sl (sl.id)}
                {@const vs = (pick.volumes || []).flatMap((v) => v.slabs).find((x) => x.id === sl.id)}
                <tr>
                  <td class="mono">{sl.id}{#if vs && vs.state !== 'ok'} <span class="warn">{vs.state}</span>{/if}</td>
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
                  <td><a href={hrefOf(v.component)}>{v.name}</a></td>
                  <td>{v.kind}</td>
                  <td>{#if v.consumer}{#if v.consumer_link}<a href={hrefOf(v.consumer_link)}>{v.consumer}</a>{:else}{v.consumer}{/if}{:else}<span class="dim">nothing</span>{/if}</td>
                  <td class="n">{formatBytes(v.bytes)}</td>
                  <td class="n" title={v.shared_legs ? `${v.shared_legs} shared with another volume (a clone and its golden)` : ''}>{v.legs}{v.shared_legs ? ` (${v.shared_legs} shared)` : ''}</td>
                  <td class:warn={v.state !== 'ok'}>{v.state}{v.rebuild && v.rebuild !== 'none' ? ` · rebuild ${v.rebuild}` : ''}{v.policy ? ` · ${v.policy}` : ''}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {:else}
          <p class="dim">
            {#if !placement}
              This node’s engine did not answer for placement{pick.local ? '' : ', and it is the only engine this console reads'}.
            {:else if placement.error}
              The engine’s placement could not be read: {placement.error}.
            {:else if placement.volumes && !placement.placed}
              This engine does not report where volumes live (placement, stormblock v17.1.0).
            {:else if !pick.local}
              No volume of this node’s engine is on it. Volumes on {pick.node}’s own engine are not read by this console.
            {:else}
              No volume has data on this drive.
            {/if}
          </p>
        {/if}
        <ResourceTable components={feed.components} rootIds={[pick.id]} {invoke} showKind={false} />
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
                          class:failing={d.health === 'error'}
                          class:sel={selected === d.id}
                          style={h.css ? `background: ${h.css}` : ''}
                          title="{d.bay !== null ? `bay ${d.bay} · ` : ''}{d.c.label} · {h.text}{d.health !== 'ok' ? ` · ${d.health}` : ''}{d.member ? ` · ${d.member}` : ''}"
                          aria-label="{d.c.label}, {h.text}"
                          onclick={() => (selected = selected === d.id ? null : d.id)}
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
              components={feed.components}
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
  .chips { display: flex; gap: 6px; margin-left: auto; }
  .chip { font-size: var(--sc-t-meta); padding: 2px 9px; border-radius: 999px; }
  .chip.error { color: var(--error); border-color: var(--error); }
  .chip.warn { color: var(--warn-strong); border-color: var(--warn-strong); }
  .chip.on { background: var(--nav-hover); font-weight: 600; }
  .bar { display: flex; flex-wrap: wrap; gap: 10px; align-items: center; margin-bottom: 10px; }
  .bar input { width: 300px; }
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
  .picked header { display: flex; gap: 10px; align-items: baseline; margin-bottom: 8px; }
  .picked header button { margin-left: auto; font-size: var(--sc-t-meta); }
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
  .drain { display: flex; flex-wrap: wrap; gap: 10px; align-items: center; margin: 6px 0; font-size: var(--sc-t-meta); }
  .pressure { margin-bottom: 10px; }
</style>
