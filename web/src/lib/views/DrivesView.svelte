<script>
  // Drives, arranged the way the hardware is (#8).
  //
  // A drive is a physical object: it lives in a shelf, in a bay, and
  // somebody eventually walks up and pulls it. Listed flat and mixed in
  // with stormblock volumes, that is unreadable at one disk and hopeless
  // at a hundred — so this groups by shelf, orders by bay, and puts the
  // shelf's own operations on the group it belongs to.
  //
  // Everything here comes from stormdrive's feed. The actions are its
  // actions — locate, join fleet, designate, format — routed through the
  // console's proxy by the feed plugin, so enrolling a drive is a button
  // rather than a shell on the node.
  import { route } from '../router.svelte.js'
  import { feed, prefs, setView } from '../stores.svelte.js'
  import { call } from '../api.js'
  import PageHeader from '../components/PageHeader.svelte'
  import Toolbar from '../components/Toolbar.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import StatusPill from '../components/StatusPill.svelte'
  import ResourceTable from '../components/ResourceTable.svelte'
  import Icon from '../components/Icon.svelte'

  // Two views of the same hardware. Drives asks "where is that disk";
  // shelves asks "which enclosure is in trouble" — a shelf fails as a
  // unit, and its PSUs and fans are the thing to look at when it does.
  const asShelves = $derived(route.current.query.get('group') === 'shelf')

  let search = $state('')
  let membership = $state('')
  let busy = $state('')

  const invoke = (a) => call(a.method, a.path)

  const drives = $derived(feed.components.filter((c) => c.kind === 'drive'))
  const shelves = $derived(feed.components.filter((c) => c.kind === 'shelf'))

  const matches = (c) => {
    if (membership === 'fleet' && !/(^|·\s)fleet(\s|·|$)/.test(c.detail || '')) return false
    if (membership === 'out' && !(c.detail || '').includes('out of fleet')) return false
    if (!search) return true
    return `${c.label} ${c.detail || ''}`.toLowerCase().includes(search.toLowerCase())
  }

  const rows = $derived(drives.filter(matches))

  /// Which shelf a drive belongs to — the `belongs_to shelf` edge
  /// stormdrive already publishes, not a guess from its name.
  function shelfOf(drive) {
    return (drive.relations || []).find((r) => r.name === 'shelf')?.targets?.[0] || null
  }

  /// The bay, so a group can be ordered the way the chassis is. stormdrive
  /// prints it in the detail line ("… DS4246 bay 4") and does not publish
  /// it as a metric yet (filed there); until it does, this reads the metric
  /// when present and the line otherwise, and drives with no bay sort last
  /// by name.
  function bayOf(drive) {
    const metric = (drive.metrics || []).find((m) => m.label === 'bay')
    if (metric) return Number(metric.value)
    const m = /bay (\d+)/.exec(drive.detail || '')
    return m ? Number(m[1]) : Number.POSITIVE_INFINITY
  }

  const groups = $derived.by(() => {
    const byShelf = new Map()
    for (const d of rows) {
      const key = shelfOf(d) || ''
      if (!byShelf.has(key)) byShelf.set(key, [])
      byShelf.get(key).push(d)
    }
    // A shelf the SES scan knows but no matching drive points at still
    // shows: an empty shelf is a fact about the rack.
    if (!search && !membership) {
      for (const s of shelves) if (!byShelf.has(s.id)) byShelf.set(s.id, [])
    }
    return [...byShelf.entries()]
      .map(([id, members]) => ({
        shelf: shelves.find((s) => s.id === id) || null,
        id,
        drives: members.sort(
          (a, b) => bayOf(a) - bayOf(b) || a.label.localeCompare(b.label)
        ),
      }))
      .sort((a, b) => {
        // Located drives first; the "no shelf" bucket is last because it
        // is the one you cannot walk up to.
        if (!a.id) return 1
        if (!b.id) return -1
        return (a.shelf?.label || a.id).localeCompare(b.shelf?.label || b.id)
      })
  })

  const inFleet = $derived(drives.filter((d) => !(d.detail || '').includes('out of fleet')).length)
  const filtered = $derived(rows.length !== drives.length)

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
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Hardware' }, { label: asShelves ? 'Shelves' : 'Drives' }]}
    title={asShelves ? 'Shelves' : 'Drives'}
    count={feed.loaded ? (asShelves ? shelves.length : drives.length) : null}
  >
    {#snippet status()}
      <span class="fleet" title="Drives enrolled in the storage fleet">
        {inFleet}/{drives.length} in fleet
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
  {:else if drives.length === 0}
    <EmptyState
      icon="storage"
      title="No drives discovered"
      hint="stormdrive on this node has not reported a disk. Either it is not running, or this machine has nothing it can see — check the Storage plugin card on the overview."
    >
      {#snippet action()}
        <a class="sc-back" href="#/">Back to overview</a>
      {/snippet}
    </EmptyState>
  {:else}
    <Toolbar
      bind:search
      placeholder="Search drives by model, serial or bay"
      bind:view={prefs.view}
      onview={setView}
      hint={filtered ? `${rows.length} of ${drives.length}` : `${groups.length} shelves`}
    >
      {#snippet filters()}
        <select bind:value={membership} aria-label="Filter by fleet membership">
          <option value="">All drives</option>
          <option value="fleet">In the fleet</option>
          <option value="out">Out of the fleet</option>
        </select>
      {/snippet}
    </Toolbar>

    {#if rows.length === 0}
      <EmptyState icon="filter" title="No matches" hint="No drive matches the current search and filter." />
    {:else}
      {#each groups as g (g.id)}
        <section class="shelf">
          <header>
            <span class="mark"><Icon name="storage" size={16} /></span>
            <h2>{g.shelf?.label || (g.id ? g.id : 'Not in a shelf')}</h2>
            {#if g.shelf}<StatusPill health={g.shelf.health} />{/if}
            <span class="detail">
              {#if g.shelf}
                {g.shelf.detail}
              {:else if !g.id}
                direct-attached — no shelf reports these
              {/if}
            </span>
            <span class="acts">
              {#each g.shelf?.metrics || [] as m}
                <span class="m"><span class="ml">{m.label}</span><span class="mv {m.tone || ''}">{m.value}{m.unit || ''}</span></span>
              {/each}
              {#each g.shelf?.actions || [] as a}
                <button
                  class:danger={a.danger}
                  disabled={!a.enabled || busy === a.id}
                  onclick={() => shelfAction(g.shelf, a)}>{a.label}</button
                >
              {/each}
            </span>
          </header>

          {#if g.drives.length === 0}
            <p class="bare">This shelf reports no drives.</p>
          {:else}
            <ResourceTable
              components={feed.components}
              rootIds={g.drives.map((d) => d.id)}
              {invoke}
              showKind={false}
            />
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

  .shelf + .shelf { margin-top: 18px; }
  .shelf header {
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
</style>
