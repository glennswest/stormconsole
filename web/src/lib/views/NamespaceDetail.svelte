<script>
  // A namespace is a place, and this is the page for it (#6).
  //
  // The property that makes it navigation rather than a summary: **every
  // count is a link**. You land here, see it holds 7 pods, click 7, and
  // you are in the pod list already filtered to this namespace. Without
  // that, "what is running in kube-system" means leaving for the workload
  // list and filtering by eye, which is what it meant before.
  import { route } from '../router.svelte.js'
  import { feed, selectNamespace, creatorsFor } from '../stores.svelte.js'
  import { get, timeAgo } from '../api.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import StatusPill from '../components/StatusPill.svelte'
  import YamlPanel from '../components/YamlPanel.svelte'
  import ResourceTable from '../components/ResourceTable.svelte'
  import CreateMenu from '../components/CreateMenu.svelte'
  import Icon from '../components/Icon.svelte'
  import { call } from '../api.js'

  const name = $derived(route.current.params.name)
  const TABS = ['Overview', 'Resources', 'Events', 'YAML']
  let tab = $state('Overview')

  let data = $state(null)
  let error = $state('')
  let loaded = $state(false)

  const component = $derived(feed.components.find((c) => c.id === `k8s:ns:${name}`))
  const resolveId = (id) => feed.components.find((c) => c.id === id)
  const invoke = (a) => call(a.method, a.path)

  // Everything the feed holds in this namespace — the Resources tab, from
  // the same components the lists are drawn from.
  const contents = $derived(
    feed.components
      .filter((c) => {
        const parts = c.id.split(':')
        return parts.length >= 3 && parts.slice(2).join(':').startsWith(`${name}/`)
      })
      .map((c) => c.id)
      .sort()
  )

  const populated = $derived((data?.inventory || []).filter((i) => i.count > 0))
  const empties = $derived((data?.inventory || []).filter((i) => i.count === 0))

  async function load(ns) {
    loaded = false
    error = ''
    try {
      data = await get(`/api/plugins/k8s/namespaces/${encodeURIComponent(ns)}`)
    } catch (e) {
      error = e.message
      data = null
    }
    loaded = true
  }

  $effect(() => {
    if (name) load(name)
  })
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Administration' }, { label: 'Namespaces', href: '#/k8s/ns' }, { label: name }]}
    title={name}
    scope={data?.created ? `created ${timeAgo(data.created)}` : ''}
  >
    {#snippet status()}
      {#if component}<StatusPill health={component.health} />{/if}
    {/snippet}
    {#snippet actions()}
      <button onclick={() => { selectNamespace(name); location.hash = '#/k8s/pod' }}>
        <Icon name="filter" size={13} /> Work in this namespace
      </button>
      <CreateMenu at={`#/k8s/ns/${name}`} primary={true} />
    {/snippet}
  </PageHeader>

  {#if !loaded}
    <div class="sc-empty"><p>Loading {name}…</p></div>
  {:else if error}
    <EmptyState
      icon="cluster"
      title="This namespace cannot be read"
      hint="The kubernetes plugin returned: {error}"
    >
      {#snippet action()}
        <a class="sc-back" href="#/k8s/ns">Back to namespaces</a>
      {/snippet}
    </EmptyState>
  {:else}
    <nav class="tabs" aria-label="Namespace sections">
      {#each TABS as t}
        <button class:active={tab === t} aria-current={tab === t ? 'page' : undefined} onclick={() => (tab = t)}>
          {t}
          {#if t === 'Events' && data.events?.length}<span class="n">{data.events.length}</span>{/if}
        </button>
      {/each}
    </nav>

    {#if tab === 'Overview'}
      <section class="cards">
        <div class="card inventory">
          <h2>Inventory</h2>
          {#if populated.length === 0}
            <p class="none">Nothing has been created in this namespace yet.</p>
          {:else}
            <ul class="counts">
              {#each populated as i (i.kind)}
                <li>
                  <a href={i.href}>
                    <span class="count">{i.count}</span>
                    <span class="what">{i.title}</span>
                  </a>
                </li>
              {/each}
            </ul>
          {/if}
          {#if empties.length}
            <p class="empties">
              No {empties.map((e) => e.title.toLowerCase()).join(', ')}.
            </p>
          {/if}
        </div>

        <div class="card">
          <h2>Quota</h2>
          {#if !data.quotas?.length}
            <!-- "No quota" is an answer, and a useful one: it means
                 nothing here is bounded. -->
            <p class="none">
              No resource quota. Nothing limits what this namespace may consume.
            </p>
          {:else}
            {#each data.quotas as q (q.name)}
              <div class="quota">
                <h3>{q.name}</h3>
                <table>
                  <thead><tr><th>Resource</th><th>Used</th><th>Hard</th></tr></thead>
                  <tbody>
                    {#each q.resources as r (r.resource)}
                      <tr><td class="mono">{r.resource}</td><td class="mono">{r.used || '—'}</td><td class="mono">{r.hard}</td></tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            {/each}
          {/if}

          <h2 class="second">Limit ranges</h2>
          {#if !data.limitRanges?.length}
            <p class="none">No limit range. Containers here may ask for anything.</p>
          {:else}
            <ul class="plain">
              {#each data.limitRanges as l (l.name)}<li class="mono">{l.name}</li>{/each}
            </ul>
          {/if}
        </div>

        <div class="card">
          <h2>Labels</h2>
          {#if Object.keys(data.labels || {}).length === 0}
            <p class="none">No labels.</p>
          {:else}
            <ul class="labels">
              {#each Object.entries(data.labels) as [k, v] (k)}
                <li class="mono">{k}={v}</li>
              {/each}
            </ul>
          {/if}
        </div>
      </section>
    {:else if tab === 'Resources'}
      {#if contents.length === 0}
        <EmptyState
          icon="inbox"
          title="Nothing in {name}"
          hint="No object in the feed belongs to this namespace yet."
        >
          {#snippet action()}
            <CreateMenu at={`#/k8s/ns/${name}`} primary={true} />
          {/snippet}
        </EmptyState>
      {:else}
        <ResourceTable components={feed.components} rootIds={contents} {invoke} />
      {/if}
    {:else if tab === 'Events'}
      {#if !data.events?.length}
        <EmptyState
          icon="events"
          title="No events in {name}"
          hint="Nothing has been reported here in the retention window. Events appear as controllers act."
        />
      {:else}
        <div class="sc-panel table-wrap">
          <table class="events">
            <thead>
              <tr><th>Last seen</th><th>Type</th><th>Reason</th><th>Object</th><th>Message</th><th>Count</th></tr>
            </thead>
            <tbody>
              {#each data.events as e}
                <tr class:warn={e.type === 'Warning'}>
                  <td class="time" title={e.time}>{e.time ? timeAgo(e.time) : '—'}</td>
                  <td><span class="type" class:warn={e.type === 'Warning'}>{e.type}</span></td>
                  <td class="reason">{e.reason}</td>
                  <td class="mono">{e.object}</td>
                  <td class="msg">{e.message}</td>
                  <td class="n">{e.count}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    {:else}
      <YamlPanel
        yaml={data.yaml}
        label="Namespace {name}"
        savePath={`/api/plugins/k8s/object/ns/${encodeURIComponent(name)}`}
      />
    {/if}
  {/if}
</div>

<style>
  .tabs {
    display: flex;
    gap: 2px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 14px;
  }
  .tabs button {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    padding: 7px 14px;
    font-size: var(--sc-t-body);
    color: var(--text-dim);
  }
  .tabs button:hover { color: var(--text); background: var(--nav-hover); }
  .tabs button.active { color: var(--text); border-bottom-color: var(--accent); font-weight: 600; }
  .tabs .n {
    margin-left: 6px;
    font-size: var(--sc-t-eyebrow);
    color: var(--text-faint);
    background: var(--panel-raised);
    border-radius: 999px;
    padding: 0 6px;
  }

  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 12px;
    align-items: start;
  }
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px var(--sc-row-px);
  }
  .card.inventory { grid-column: span 1; }
  h2 {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin: 0 0 10px;
  }
  h2.second { margin-top: 18px; }
  h3 { font-size: var(--sc-t-body); margin: 0 0 6px; font-weight: 600; }
  .none { margin: 0; font-size: var(--sc-t-body); color: var(--text-dim); }
  .empties { margin: 10px 0 0; font-size: var(--sc-t-meta); color: var(--text-faint); }

  /* The inventory is the navigation: each count is the link. */
  .counts { list-style: none; display: grid; gap: 2px; }
  .counts a {
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 5px 8px;
    border-radius: var(--radius-sm);
    color: var(--text);
  }
  .counts a:hover { background: var(--nav-hover); text-decoration: none; }
  .count {
    min-width: 2.2em;
    text-align: right;
    font-family: var(--mono);
    font-size: 15px;
    font-weight: 600;
    color: var(--accent);
    font-variant-numeric: tabular-nums;
  }
  .what { font-size: var(--sc-t-body); }

  .plain, .labels { list-style: none; display: grid; gap: 3px; }
  .labels li, .plain li { font-size: var(--sc-t-meta); color: var(--text-dim); }

  .quota + .quota { margin-top: 12px; }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  th {
    text-align: left;
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-faint);
    font-weight: 600;
    padding: 3px 0;
  }
  td { padding: 3px 0; border-top: 1px solid var(--sc-hairline); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }

  .table-wrap { overflow-x: auto; }
  .events th {
    background: color-mix(in srgb, var(--panel-raised) 55%, var(--panel));
    padding: var(--sc-row-py) var(--sc-row-px);
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
    text-transform: none;
    letter-spacing: 0;
    font-size: var(--sc-t-meta);
    color: var(--text-dim);
  }
  .events td {
    padding: var(--sc-row-py) var(--sc-row-px);
    border-top: none;
    border-bottom: 1px solid var(--sc-hairline);
    vertical-align: top;
  }
  .type {
    font-size: var(--sc-t-eyebrow);
    font-weight: 600;
    padding: 1px 8px;
    border-radius: 999px;
    border: 1px solid var(--border);
    color: var(--text-faint);
    white-space: nowrap;
  }
  .type.warn { color: var(--warn-strong); border-color: var(--warn-border); background: var(--warn-bg); }
  .reason { font-weight: 500; white-space: nowrap; }
  .msg { color: var(--text-dim); min-width: 260px; }
  .time { white-space: nowrap; color: var(--text-dim); }
  .n { text-align: right; color: var(--text-dim); }
  .sc-back { font-size: var(--sc-t-body); }
</style>
