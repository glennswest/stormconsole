<script>
  // The console's front door: how the whole feed is doing, then each
  // plugin's own summary and its objects underneath. Holds no model of
  // its own — everything here is the aggregated feed, grouped.
  import { feed, rollup, prefs, setView } from '../stores.svelte.js'
  import ComponentCard from 'stormview/components/ComponentCard.svelte'
  import ResourceTable from '../components/ResourceTable.svelte'
  import PageHeader from '../components/PageHeader.svelte'
  import StatusPill from '../components/StatusPill.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import Icon from '../components/Icon.svelte'
  import { call } from '../api.js'

  const PREVIEW = 8
  const STATES = [
    { key: 'ok', label: 'Ready' },
    { key: 'warn', label: 'Degraded' },
    { key: 'error', label: 'Failed' },
    { key: 'idle', label: 'Idle' },
    { key: 'unknown', label: 'Unknown' },
  ]

  const resolveId = (id) => feed.components.find((c) => c.id === id)
  const invoke = (a) => call(a.method, a.path)

  let filter = $state('')
  let showAll = $state({})

  const health = $derived(rollup())
  const plugins = $derived(feed.components.filter((c) => c.kind === 'plugin'))

  const groups = $derived(
    plugins.map((p) => {
      const items = feed.components.filter(
        (c) => c.kind !== 'plugin' && c.id.startsWith(p.label + ':')
      )
      return {
        card: p,
        all: items,
        items: filter ? items.filter((c) => c.health === filter) : items,
      }
    })
  )

  const shown = $derived(groups.reduce((n, g) => n + g.items.length, 0))

  function toggle(key) {
    filter = filter === key ? '' : key
  }
</script>

<div class="sc-page">
  <PageHeader title="Overview" count={feed.loaded ? health.total : null}>
    {#snippet actions()}
      <span class="sc-seg" role="group" aria-label="View">
        <button aria-pressed={prefs.view === 'table'} title="Table view" onclick={() => setView('table')}>
          <Icon name="table" size={14} />
        </button>
        <button aria-pressed={prefs.view === 'cards'} title="Card view" onclick={() => setView('cards')}>
          <Icon name="cards" size={14} />
        </button>
      </span>
    {/snippet}
  </PageHeader>

  {#if !feed.loaded}
    <div class="sc-empty"><p>Connecting to the component feed…</p></div>
  {:else if health.total === 0}
    <EmptyState
      icon="cluster"
      title="No components reporting"
      hint="Every plugin is mounted but nothing upstream has answered yet. Check that the node's services are running."
    />
  {:else}
    <!-- The status band: the whole fleet's health in one line, and a
         filter — clicking a state narrows everything below it. -->
    <section class="band sc-panel" aria-label="Fleet health">
      <div class="states">
        {#each STATES as s}
          {#if health[s.key]}
            <button
              class="state"
              class:on={filter === s.key}
              aria-pressed={filter === s.key}
              onclick={() => toggle(s.key)}
            >
              <span class="n sc-num">{health[s.key]}</span>
              <StatusPill health={s.key} label={s.label} />
            </button>
          {/if}
        {/each}
        {#if filter}
          <button class="clear" onclick={() => (filter = '')}>Clear filter</button>
        {/if}
      </div>
      <div class="dist" aria-hidden="true">
        {#each STATES as s}
          {#if health[s.key]}
            <span
              class="seg {s.key}"
              style="flex: {health[s.key]}"
              title="{health[s.key]} {s.label.toLowerCase()}"
            ></span>
          {/if}
        {/each}
      </div>
    </section>

    <h2 class="eyebrow">Plugins</h2>
    <div class="grid sc-cards">
      {#each plugins as p (p.id)}
        <ComponentCard component={p} resolve={resolveId} {invoke} />
      {/each}
    </div>

    {#if filter && shown === 0}
      <EmptyState
        icon="filter"
        title="Nothing is {STATES.find((s) => s.key === filter)?.label.toLowerCase()}"
        hint="No component in the feed is in that state right now."
      />
    {/if}

    {#each groups as g (g.card.id)}
      {#if g.items.length}
        {@const open = showAll[g.card.id]}
        {@const list = open ? g.items : g.items.slice(0, PREVIEW)}
        <h2 class="eyebrow">
          {g.card.label}
          <span class="sc-count">{g.items.length}</span>
          {#if g.items.length > PREVIEW}
            <button class="more" onclick={() => (showAll[g.card.id] = !open)}>
              {open ? 'Show less' : `Show all ${g.items.length}`}
            </button>
          {/if}
        </h2>
        {#if prefs.view === 'cards'}
          <div class="grid sc-cards">
            {#each list as c (c.id)}
              <ComponentCard component={c} resolve={resolveId} {invoke} />
            {/each}
          </div>
        {:else}
            <ResourceTable components={feed.components} rootIds={list.map((c) => c.id)} {invoke} />
        {/if}
      {/if}
    {/each}
  {/if}
</div>

<style>
  .band {
    display: grid;
    gap: 14px;
    padding: 16px 18px;
    margin-bottom: 22px;
  }
  .states { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .state {
    display: inline-flex;
    align-items: baseline;
    gap: 8px;
    padding: 6px 12px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius);
  }
  .state:hover { background: var(--nav-hover); border-color: var(--border); }
  .state.on { background: var(--nav-active); border-color: var(--border-strong); }
  .n {
    font-size: 26px;
    font-weight: 600;
    letter-spacing: -0.02em;
    color: var(--text);
    line-height: 1;
  }
  .clear { margin-left: 4px; font-size: var(--sc-t-meta); padding: 4px 10px; }

  /* One rule, proportional: the shape of the fleet at a glance. */
  .dist {
    display: flex;
    gap: 2px;
    height: 6px;
    border-radius: 3px;
    overflow: hidden;
  }
  .seg { display: block; border-radius: 2px; }
  .seg.ok { background: var(--ok); }
  .seg.warn { background: var(--warn-strong); }
  .seg.error { background: var(--error); }
  .seg.idle { background: var(--text-ghost); }
  .seg.unknown { background: var(--border-strong); }

  .eyebrow {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: var(--sc-t-eyebrow);
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: var(--text-faint);
    margin: 24px 0 10px;
  }
  .eyebrow:first-of-type { margin-top: 0; }
  .more {
    margin-left: auto;
    font-size: var(--sc-t-meta);
    text-transform: none;
    letter-spacing: 0;
    font-weight: 500;
    padding: 3px 10px;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 12px;
  }
</style>
