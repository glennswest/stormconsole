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
  // The model is drivemap.js, tested at 1,600 drives without a browser.
  import { route } from '../router.svelte.js'
  import { feed } from '../stores.svelte.js'
  import { call } from '../api.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import StatusPill from '../components/StatusPill.svelte'
  import ResourceTable from '../components/ResourceTable.svelte'
  import Icon from '../components/Icon.svelte'
  import { build, groups as groupBy, totals, heat, cells, columns, formatBytes, FILTERS, MODES } from '../drivemap.js'

  const asShelves = $derived(route.current.query.get('group') === 'shelf')

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

  const model = $derived(build(feed.components))
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
    crumbs={[{ label: 'Hardware' }, { label: asShelves ? 'Shelves' : 'Drives' }]}
    title={asShelves ? 'Shelves' : 'Drives'}
    count={feed.loaded ? (asShelves ? shelves.length : records.length) : null}
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
        <span class="big" title="Of the drives whose usage is known: this node's, from its engine">{formatBytes(all.used)} <small>used of {formatBytes(all.slab)} in slabs</small></span>
      {/if}
      <span class="chips">
        {#each [['failing', all.failing, 'error'], ['degraded', all.degraded, 'warn'], ['rebuilding', all.rebuilding, 'warn'], ['full', all.full, 'warn'], ['hot', all.hot, 'warn']] as [f, n, tone]}
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
          {#if mode === 'usage'}<span class="dim">usage is this node's, from its engine, until stormdrive reports it (stormdrive#12)</span>{/if}
        {/if}
        <span><i class="ring"></i>rebuilding</span>
      </div>
    {/if}

    {#if pick}
      <section class="picked">
        <header>
          <strong>{pick.c.label}</strong>
          <span class="dim">{pick.node}{pick.shelfLabel ? ` · ${pick.shelfLabel}` : ''}{pick.bay !== null ? ` · bay ${pick.bay}` : ''}</span>
          <button onclick={() => (selected = null)}>Close</button>
        </header>
        {#if pick.usage}
          {@const u = pick.usage}
          <div class="usebar" title="used / free in slabs / not in a slab">
            <span class="used" style="width: {(u.used / pick.capacity) * 100}%"></span>
            <span class="free" style="width: {(u.free / pick.capacity) * 100}%"></span>
          </div>
          <p class="dim">{formatBytes(u.used)} used · {formatBytes(u.free)} free in slabs · {formatBytes(u.unslabbed)} not in a slab · of {formatBytes(pick.capacity)}</p>
        {:else}
          <p class="dim">{pick.local ? 'The engine holds no slab on this drive.' : 'Usage is read from this node’s engine only, until stormdrive reports it (stormdrive#12).'}</p>
        {/if}
        {#if pick.member}<p class="warn">Array member: {pick.member}</p>{/if}
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
</style>
