<script>
  // Images: the registry's catalog (#19). Goldens, blanks and media are
  // images, and they are shown here — grouped by kind, each with the base it
  // is built on (the Base column is its lineage), how many volumes were
  // cloned from it, which releases carry it, and, for one still arriving,
  // where it is coming from and how far along it is. They stay the engine's
  // volumes underneath; nothing moved. What is *attached* to something
  // running is under Storage → Volumes.
  import { feed } from '../stores.svelte.js'
  import { call } from '../api.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import ResourceTable from '../components/ResourceTable.svelte'

  const KINDS = [
    ['component', 'Component goldens', 'what a node runs a service from'],
    ['blank', 'Blanks', 'empty filesystems a claim or a component’s data is cut from'],
    ['media', 'VM media', 'cloud images and ISOs a machine boots from'],
    ['golden', 'Image goldens', 'a container image, made a volume'],
    ['base', 'Bases', 'shared layers goldens are built on'],
    ['slab_golden', 'Slab goldens', 'members of a node’s system slab'],
    ['release_part', 'Release parts', 'what a published release is composed of'],
    ['sealed', 'Other sealed volumes', 'sealed, and nothing here knows what they are'],
  ]

  let search = $state('')
  let location = $state('')
  const invoke = (a) => call(a.method, a.path)

  const kindOf = (c) => c.metrics?.find((m) => m.label === 'kind')?.value || 'sealed'
  const locOf = (c) => c.metrics?.find((m) => m.label === 'location')?.value || 'local'

  const all = $derived(feed.components.filter((c) => c.kind === 'catalog-image' || c.kind === 'media-job'))
  const locations = $derived([...new Set(all.map(locOf))].sort())
  const shown = $derived(
    all.filter(
      (c) =>
        (!location || locOf(c) === location) &&
        (!search || `${c.label} ${c.detail}`.toLowerCase().includes(search.toLowerCase()))
    )
  )
  const groups = $derived(
    KINDS.map(([k, title, what]) => ({
      k,
      title,
      what,
      ids: shown.filter((c) => kindOf(c) === k).sort((a, b) => a.label.localeCompare(b.label)).map((c) => c.id),
    })).filter((g) => g.ids.length)
  )
  const registry = $derived(feed.components.find((c) => c.id === 'reg:registry'))
  const arriving = $derived(all.filter((c) => c.health === 'warn' || c.kind === 'media-job').length)
</script>

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Images' }, { label: 'Catalog' }]} title="Images" count={feed.loaded ? all.length : null} />

  <p class="lead">
    Everything a machine, a claim or a service is cloned from — held by the registry, stored as the
    engine’s sealed volumes. The Base column is each one’s lineage; Clones is how many volumes were cut
    from it.
  </p>

  <div class="bar">
    <input bind:value={search} placeholder="Search images" aria-label="Search images" />
    {#if locations.length > 1}
      <select bind:value={location} aria-label="Location">
        <option value="">Everywhere</option>
        {#each locations as l}<option value={l}>{l}</option>{/each}
      </select>
    {/if}
    {#if arriving}<span class="note">{arriving} arriving or failed</span>{/if}
  </div>

  {#if !feed.loaded}
    <div class="sc-empty"><p>Connecting to the component feed…</p></div>
  {:else if !all.length}
    <EmptyState
      icon="image"
      title="No images"
      hint={registry
        ? `The registry says: ${registry.detail}`
        : 'No registry is configured for this console ([sbregistry] url).'}
    />
  {:else}
    {#each groups as g (g.k)}
      <section class="group">
        <h2>{g.title} <span class="count">{g.ids.length}</span> <span class="what">{g.what}</span></h2>
        <ResourceTable components={feed.components} rootIds={g.ids} {invoke} />
      </section>
    {/each}
  {/if}
</div>

<style>
  .lead { font-size: var(--sc-t-body); color: var(--text-dim); max-width: 80ch; margin: 0 0 12px; }
  .bar { display: flex; gap: 10px; align-items: center; margin-bottom: 12px; flex-wrap: wrap; }
  .bar input { width: 280px; }
  .note { color: var(--warn-strong); font-size: var(--sc-t-meta); }
  .group { margin-bottom: 18px; }
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0 0 6px; }
  .count { color: var(--text-dim); margin-left: 4px; }
  .what { text-transform: none; letter-spacing: 0; font-weight: 400; color: var(--text-faint); margin-left: 8px; }
</style>
