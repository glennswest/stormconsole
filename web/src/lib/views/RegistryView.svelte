<script>
  // The registry, for a forge most of all (#163): every repository with its tags and
  // digests, and who holds a clone of each. The owner: "a registry ui with a list of
  // images/versions, and who has goldens", in OCI Distribution's words.
  //
  // Read with the console's own engine credential, on the server; a holder in a
  // project this viewer may not see is counted and not named.
  import { onDestroy } from 'svelte'
  import { get } from '../api.js'
  import { route, navigate } from '../router.svelte.js'
  import { ago } from '../ui/time.js'
  import { holderWord, holderName, imageKind, ref, shortDigest, holderCounts, narrowed, byHolder, bytes } from '../registry.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import CopyButton from '../components/CopyButton.svelte'

  const q = route.current.query
  const view = q.get('view') === 'holders' ? 'holders' : 'repositories'
  const repo = q.get('repo') || ''
  let search = $state(q.get('search') || '')
  let kind = $state(q.get('holder') || '')

  let data = $state(null)
  let error = $state('')
  let timer = null
  async function load() {
    try {
      data = await get('/api/plugins/reg/holdings')
      error = ''
    } catch (e) {
      error = `the console did not answer: ${e.message}`
    }
    timer = setTimeout(load, 15000)
  }
  load()
  onDestroy(() => clearTimeout(timer))

  const h = $derived(data?.holdings)
  const counts = $derived(holderCounts(h))
  const repos = $derived(narrowed(h, { search, kind, repo }))
  const rows = $derived(byHolder(h, { search, kind }))
  const link = (params) => `#/registry?${new URLSearchParams(params)}`
  const claimed = (s) => (s ? `${ago(new Date(s * 1000).toISOString())} ago` : '')
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Images' }, { label: 'Registry', href: '#/registry' }, ...(repo ? [{ label: repo }] : [])]}
    title={repo || 'Registry'}
    scope="this node's engine"
    count={h ? (repo ? repos[0]?.images.length ?? 0 : h.repositories.length) : null}
  />

  {#if error}<p class="bad">{error}</p>{/if}
  {#if data?.engine_error}
    <EmptyState icon="storage" title="The engine could not be read" hint={data.engine_error} />
  {:else if h}
    {#if data.catalog_note}
      <p class="warn small">Repositories and tags are worked out from volume names, with no digests or releases: {data.catalog_note}.</p>
    {/if}
    {#if data.hosts_note}
      <p class="warn small">Boot clones may read as nobody's, because the machines could not be read: {data.hosts_note}.</p>
    {/if}
    <p class="band">
      <span><strong>{h.repositories.length}</strong> repositories</span>
      <span><strong>{h.images}</strong> tags</span>
      <span><strong>{h.clones}</strong> clones</span>
      {#if h.orphans}<a class="warn" href={link({ holder: 'nobody' })}><strong>{h.orphans}</strong> held by nobody</a>{/if}
    </p>

    <nav class="tabs" aria-label="Registry views">
      <button class:active={view === 'repositories'} onclick={() => navigate('#/registry')}>Repositories</button>
      <button class:active={view === 'holders'} onclick={() => navigate('#/registry?view=holders')}>By holder</button>
    </nav>

    <div class="bar">
      <input bind:value={search} placeholder={view === 'holders' ? 'A machine, a pod, a tag' : 'A repository, a tag, a digest, a holder'} aria-label="Search" />
      <div class="chips" role="group" aria-label="Held by">
        <button class:on={!kind} onclick={() => (kind = '')}>All <span class="n">{h.clones}</span></button>
        {#each counts as c (c.kind)}
          <button class:on={kind === c.kind} class:warnb={c.kind === 'nobody'} onclick={() => (kind = kind === c.kind ? '' : c.kind)}>{c.label} <span class="n">{c.count}</span></button>
        {/each}
      </div>
    </div>

    {#if view === 'repositories'}
      {#if !h.repositories.length}
        <EmptyState icon="storage" title="No registry images" hint="This node's engine holds no sealed volume." />
      {:else if !repos.length}
        <p class="dim">Nothing matches.</p>
      {/if}
      {#each repos as r (r.name)}
        <section class="card">
          <header>
            <h2><a href={link({ repo: r.name })}>{r.name}</a></h2>
            <span class="dim small">{r.images.length} {r.images.length === 1 ? 'tag' : 'tags'} · {r.clones} {r.clones === 1 ? 'clone' : 'clones'}</span>
            {#if r.orphans}<span class="warn small">{r.orphans} held by nobody</span>{/if}
          </header>
          {#each r.images as i (i.id)}
            <div class="image">
              <div class="head">
                <strong class="mono">{i.tag || '(no tag)'}</strong>
                <span class="tagk">{imageKind(i.kind)}</span>
                {#if i.digest}<span class="mono dim" title={i.digest}>{shortDigest(i.digest)}</span> <CopyButton value={i.digest} />{/if}
                <span class="dim small">{bytes(i.virtual_size_bytes)}, {bytes(i.allocated_bytes)} used</span>
                {#if i.releases?.length}<span class="small">in {i.releases.join(', ')}</span>{/if}
                {#if i.base}<span class="dim small">built on {i.base}</span>{/if}
                {#if i.built_on_it}<span class="dim small">{i.built_on_it} built on it</span>{/if}
                <span class="dim small volume" title="the engine's volume">{i.name}</span>
              </div>
              {#if i.clones.length}
                <table>
                  <thead><tr><th>Held by</th><th></th><th>Clone</th><th>Beyond the image</th><th>Serving</th></tr></thead>
                  <tbody>
                    {#each i.clones as c (c.id)}
                      <tr class:alert={c.holder.kind === 'nobody'}>
                        <td class="kind" class:warn={c.holder.kind === 'nobody'}>{holderWord(c.holder.kind)}</td>
                        <td>
                          {#if c.holder.kind === 'nobody'}<span class="dim">{c.holder.detail}</span>
                          {:else}{holderName(c.holder)}{#if c.holder.detail} <span class="dim small">{c.holder.detail}</span>{/if}{#if c.holder.claimed_at} <span class="dim small">claimed {claimed(c.holder.claimed_at)}</span>{/if}{/if}
                        </td>
                        <td class="mono">{c.name}{#if c.via?.length} <span class="dim">through {c.via.join(', ')}</span>{/if}</td>
                        <td class="num">{bytes(c.allocated_bytes)}</td>
                        <td class="small">{c.in_use ? 'yes' : ''}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              {:else if !i.all}
                <p class="dim small none">Nobody holds a clone of it.</p>
              {/if}
            </div>
          {/each}
        </section>
      {/each}
    {:else}
      {#if !rows.length}
        <p class="dim">{search || kind ? 'Nothing matches.' : 'No clones on this engine.'}</p>
      {:else}
        <div class="table-wrap">
          <table class="flat">
            <thead><tr><th>Held by</th><th></th><th>Runs on</th><th>Digest</th><th>Clone</th><th>Beyond the image</th></tr></thead>
            <tbody>
              {#each rows as c (c.id)}
                <tr class:alert={c.holder.kind === 'nobody'}>
                  <td class="kind" class:warn={c.holder.kind === 'nobody'}>{holderWord(c.holder.kind)}</td>
                  <td>{#if c.holder.kind === 'nobody'}<span class="dim">{c.holder.detail}</span>{:else}{holderName(c.holder)}{#if c.holder.detail} <span class="dim small">{c.holder.detail}</span>{/if}{#if c.holder.claimed_at} <span class="dim small">claimed {claimed(c.holder.claimed_at)}</span>{/if}{/if}</td>
                  <td>{#if c.image}<a class="mono" href={link({ repo: c.image.repository })}>{ref(c.image)}</a>{:else}<span class="warn small">its parent is gone from the engine</span>{/if}</td>
                  <td class="mono dim">{shortDigest(c.image?.digest)}</td>
                  <td class="mono">{c.name}</td>
                  <td class="num">{bytes(c.allocated_bytes)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    {/if}
  {/if}
</div>

<style>
  .small { font-size: var(--sc-t-meta); }
  .dim { color: var(--text-dim); }
  .warn { color: var(--warn-strong); }
  .bad { color: var(--error); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .band { display: flex; gap: 18px; flex-wrap: wrap; margin: 0 0 8px; }
  .band strong { font-size: 1.15em; font-variant-numeric: tabular-nums; }
  .tabs { display: flex; gap: 2px; border-bottom: 1px solid var(--border); margin-bottom: 12px; }
  .tabs button { background: none; border: 0; border-bottom: 2px solid transparent; border-radius: 0; padding: 8px 14px; color: var(--text-dim); cursor: pointer; font: inherit; }
  .tabs button:hover { color: var(--text); background: var(--nav-hover); }
  .tabs button.active { color: var(--text); border-bottom-color: var(--accent); font-weight: 600; }
  .bar { display: flex; gap: 12px; align-items: center; flex-wrap: wrap; margin-bottom: 12px; }
  .bar input { width: 300px; max-width: 100%; }
  .chips { display: flex; gap: 6px; flex-wrap: wrap; }
  .chips button { font-size: var(--sc-t-meta); padding: 2px 10px; border-radius: 999px; }
  .chips button.on { background: var(--nav-active); color: var(--text); border-color: var(--accent); }
  .chips button.warnb { color: var(--warn-strong); }
  .chips .n { color: var(--text-faint); font-variant-numeric: tabular-nums; margin-left: 2px; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 12px var(--sc-row-px); margin-bottom: 12px; min-width: 0; }
  .card header { display: flex; gap: 12px; align-items: baseline; flex-wrap: wrap; margin-bottom: 6px; }
  h2 { font-size: var(--sc-t-body); margin: 0; font-weight: 600; }
  .image { border-top: 1px solid var(--sc-hairline); padding: 8px 0; min-width: 0; }
  .head { display: flex; gap: 10px; align-items: baseline; flex-wrap: wrap; margin-bottom: 6px; }
  .volume { margin-left: auto; }
  .tagk { font-size: var(--sc-t-eyebrow); padding: 1px 6px; border-radius: 999px; border: 1px solid var(--border); color: var(--text-dim); white-space: nowrap; }
  .none { margin: 0; }
  .table-wrap { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  table.flat { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); }
  th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); padding: 5px 10px; border-bottom: 1px solid var(--border); }
  td { padding: 5px 10px; vertical-align: top; overflow-wrap: anywhere; }
  tr + tr > td { border-top: 1px solid var(--sc-hairline); }
  tr.alert td:first-child { box-shadow: inset 3px 0 var(--warn-strong); }
  td.kind { white-space: nowrap; }
  td.num { font-variant-numeric: tabular-nums; font-family: var(--mono); font-size: var(--sc-t-meta); white-space: nowrap; }
</style>
