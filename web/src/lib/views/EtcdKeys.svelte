<script>
  // The datastore's keyspace, as directories.
  //
  // etcd's keyspace is flat; `/registry/pods/default/web-1` is one key, not
  // a path. Grouping on the next `/` turns it into the tree everybody reads
  // it as anyway, with a count at each level — which is most of what anyone
  // opens it for ("how many events are there?"). A leaf opens its value,
  // decoded: JSON as JSON, a Kubernetes protobuf envelope by the type it
  // names, text as text, anything else as the bytes it is.
  //
  // Read-only, and admin only: this is every object in the cluster beneath
  // Kubernetes RBAC, Secrets included. The server refuses anyone else; the
  // page says why rather than rendering an empty tree.
  import { route } from '../router.svelte.js'
  import { get } from '../api.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'

  const prefix = $derived(route.current.query.get('prefix') || '/registry/')
  const key = $derived(route.current.query.get('key'))

  let listing = $state(null)
  let value = $state(null)
  let error = $state('')
  let filter = $state('')

  async function load(url) {
    const resp = await fetch(url)
    const data = await resp.json().catch(() => ({}))
    if (!resp.ok) throw new Error(data.error || `${resp.status} ${resp.statusText}`)
    return data
  }

  $effect(() => {
    const p = prefix
    const k = key
    error = ''
    listing = null
    value = null
    load(`/api/plugins/etcd/keys?prefix=${encodeURIComponent(p)}`)
      .then((d) => (listing = d))
      .catch((e) => (error = e.message))
    if (k) {
      load(`/api/plugins/etcd/value?key=${encodeURIComponent(k)}`)
        .then((d) => (value = d))
        .catch((e) => (error = e.message))
    }
  })

  const href = (p, k) =>
    `#/etcd/keys?prefix=${encodeURIComponent(p)}${k ? `&key=${encodeURIComponent(k)}` : ''}`

  // /registry/pods/default/ → [/, registry/, pods/, default/], each a link.
  const crumbs = $derived.by(() => {
    const parts = prefix.split('/').filter(Boolean)
    let acc = '/'
    return parts.map((p) => {
      acc += `${p}/`
      return { label: p, path: acc }
    })
  })

  const rows = $derived(
    (listing?.children || []).filter(
      (c) => !filter || c.name.toLowerCase().includes(filter.toLowerCase())
    )
  )

  const shown = (d) => {
    if (!d) return ''
    if (d.json !== undefined) return JSON.stringify(d.json, null, 2)
    if (d.text !== undefined) return d.text
    return d.hex || ''
  }
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Datastore' }, { label: 'fastetcd', href: '#/grid?id=etcd:store' }, { label: 'Keyspace' }]}
    title="Keyspace"
    count={listing ? listing.total : null}
  >
    {#snippet actions()}
      <a class="dl" href="/api/plugins/etcd/snapshot" download title="The whole store, as etcdctl snapshot save writes it">
        Download snapshot
      </a>
    {/snippet}
  </PageHeader>

  <nav class="path" aria-label="Prefix">
    <a href={href('/')}>/</a>
    {#each crumbs as c (c.path)}
      <a href={href(c.path)}>{c.label}/</a>
    {/each}
  </nav>

  {#if error}
    <EmptyState icon="filter" title="The keyspace cannot be read" hint={error} />
  {:else if !listing}
    <div class="sc-empty"><p>Reading keys under {prefix}…</p></div>
  {:else}
    <div class="split" class:open={!!key}>
      <section class="list">
        <input class="sc-search" placeholder="Filter this level" bind:value={filter} aria-label="Filter keys" />
        {#if listing.truncated}
          <p class="note">
            {listing.total} keys under this prefix; the first {listing.scanned} are grouped here. Go a level
            deeper for the rest.
          </p>
        {/if}
        {#if rows.length === 0}
          <p class="note">No keys under {prefix}.</p>
        {:else}
          <table>
            <thead><tr><th>Name</th><th class="n">Keys</th></tr></thead>
            <tbody>
              {#each rows as c (c.path)}
                <tr class:sel={c.path === key}>
                  <td>
                    {#if c.leaf}
                      <a href={href(prefix, c.path)}>{c.name}</a>
                    {:else}
                      <a class="dir" href={href(c.path)}>{c.name}</a>
                    {/if}
                  </td>
                  <td class="n">{c.count}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </section>

      {#if key}
        <section class="value">
          <header>
            <code>{key}</code>
            <a class="close" href={href(prefix)} aria-label="Close">×</a>
          </header>
          {#if !value}
            <p class="note">Reading…</p>
          {:else}
            <dl>
              <dt>Encoding</dt><dd>{value.decoded.encoding}</dd>
              {#if value.decoded.kind}<dt>Kind</dt><dd>{value.decoded.api_version || ''} {value.decoded.kind}</dd>{/if}
              <dt>Size</dt><dd>{value.size} bytes</dd>
              <dt>Modified at</dt><dd>revision {value.mod_revision} (version {value.version})</dd>
              <dt>Created at</dt><dd>revision {value.create_revision}</dd>
              {#if value.lease}<dt>Lease</dt><dd>{value.lease}</dd>{/if}
            </dl>
            {#if value.decoded.note}<p class="note">{value.decoded.note}</p>{/if}
            <pre>{shown(value.decoded)}</pre>
          {/if}
        </section>
      {/if}
    </div>
  {/if}
</div>

<style>
  .path {
    display: flex;
    flex-wrap: wrap;
    gap: 2px;
    font-family: var(--font-mono, monospace);
    font-size: var(--sc-t-body);
    margin-bottom: 12px;
  }
  .path a { color: var(--accent); text-decoration: none; }
  .split { display: grid; grid-template-columns: 1fr; gap: 16px; }
  .split.open { grid-template-columns: minmax(240px, 1fr) 2fr; }
  @media (max-width: 800px) { .split.open { grid-template-columns: 1fr; } }
  .list .sc-search { width: 100%; margin-bottom: 8px; }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  th { text-align: left; color: var(--text-faint); font-weight: 500; padding: 4px 8px; border-bottom: 1px solid var(--border); }
  td { padding: var(--sc-row-py, 6px) 8px; border-bottom: 1px solid var(--border); }
  td a { color: var(--text); text-decoration: none; font-family: var(--font-mono, monospace); }
  td a.dir { color: var(--accent); }
  tr.sel td { background: var(--panel); }
  .n { text-align: right; font-variant-numeric: tabular-nums; width: 6em; }
  .note { color: var(--text-faint); font-size: var(--sc-t-meta); margin: 4px 0 8px; }
  .value { min-width: 0; border: 1px solid var(--border); border-radius: 4px; padding: 12px; background: var(--panel); }
  .value header { display: flex; justify-content: space-between; gap: 8px; margin-bottom: 8px; }
  .value code { overflow-wrap: anywhere; }
  .close { color: var(--text-faint); text-decoration: none; font-size: 1.2em; }
  dl { display: grid; grid-template-columns: max-content 1fr; gap: 2px 12px; font-size: var(--sc-t-meta); margin: 0 0 8px; }
  dt { color: var(--text-faint); }
  dd { margin: 0; }
  pre {
    margin: 0;
    max-height: 60vh;
    overflow: auto;
    font-size: var(--sc-t-meta);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .dl { font-size: var(--sc-t-body); color: var(--accent); text-decoration: none; }
</style>
