<script>
  // The cluster overview: the aggregated feed grouped by plugin card, with
  // each plugin's components under it. Holds no model of its own.
  import { feed } from '../stores.svelte.js'
  import ComponentCard from 'stormview/components/ComponentCard.svelte'
  import { post } from '../api.js'

  const resolveId = (id) => feed.components.find((c) => c.id === id)

  let plugins = $derived(feed.components.filter((c) => c.kind === 'plugin'))
  let byPlugin = $derived(
    plugins.map((p) => ({
      card: p,
      items: feed.components.filter(
        (c) => c.kind !== 'plugin' && c.id.startsWith(p.label + ':')
      ),
    }))
  )
</script>

<div class="content">
  {#if !feed.loaded}
    <div class="empty">Connecting…</div>
  {:else}
    {#each byPlugin as group (group.card.id)}
      <h2>{group.card.label}</h2>
      <div class="grid">
        <ComponentCard component={group.card} resolve={resolveId} invoke={(a) => post(a.path)} />
        {#each group.items as c (c.id)}
          <ComponentCard component={c} resolve={resolveId} invoke={(a) => post(a.path)} />
        {/each}
      </div>
    {/each}
  {/if}
</div>

<style>
  h2 {
    font-size: 13px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-faint);
    margin: 18px 0 10px;
  }
  h2:first-of-type { margin-top: 0; }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 12px;
  }
  .empty { color: var(--text-faint); padding: 40px; text-align: center; }
</style>
