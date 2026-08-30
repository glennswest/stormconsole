<script>
  // A grid rooted at one component — where a nav item or a card's ⊞ lands.
  // With ?rel= the top rows are that relationship's targets; without it,
  // the component itself is the single expandable root.
  import { route } from '../router.svelte.js'
  import { feed } from '../stores.svelte.js'
  import ComponentGrid from 'stormview/components/ComponentGrid.svelte'
  import HealthDot from 'stormview/components/HealthDot.svelte'
  import { call } from '../api.js'
  import CreateMenu from '../components/CreateMenu.svelte'

  const id = $derived(route.current.query.get('id'))
  const rel = $derived(route.current.query.get('rel'))
  const root = $derived(feed.components.find((c) => c.id === id))

  // With ?rel=, an absent relation means "none yet" — an honest empty
  // list with a way to create, not the root card standing in for it.
  const rootIds = $derived.by(() => {
    if (!root) return []
    if (rel) {
      const r = (root.relations || []).find((x) => x.name === rel)
      return r ? r.targets : []
    }
    return [root.id]
  })
  const hash = $derived(`#/grid?id=${id}${rel ? `&rel=${rel}` : ''}`)
</script>

<div class="content">
  {#if !feed.loaded}
    <div class="empty">Connecting…</div>
  {:else if !root}
    <div class="empty">Component “{id}” not found. <a href="#/">Back to overview</a></div>
  {:else}
    <div class="head">
      <a href="#/" class="back">← Overview</a>
      <h1>
        <HealthDot health={root.health} />
        {root.label}
        {#if rel}<span class="rel">· {rel}</span>{/if}
        <span class="count">{rootIds.length}</span>
      </h1>
      <span class="tools"><CreateMenu at={hash} /></span>
    </div>
    {#if rootIds.length === 0}
      <div class="empty">
        No {rel || root.label} yet.
        <div class="empty-create"><CreateMenu at={hash} primary={true} /></div>
      </div>
    {:else}
      <ComponentGrid components={feed.components} {rootIds} invoke={(a) => call(a.method, a.path)} />
    {/if}
  {/if}
</div>

<style>
  .head { display: flex; align-items: baseline; gap: 16px; margin-bottom: 14px; }
  .back { font-size: 13px; color: var(--text-dim); }
  .back:hover { color: var(--accent); text-decoration: none; }
  h1 {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 16px;
    font-weight: 600;
  }
  .rel { color: var(--text-dim); font-weight: 400; }
  .count {
    font-size: 12px; color: var(--text-faint); background: var(--panel-raised);
    border: 1px solid var(--border); border-radius: 10px; padding: 1px 8px; font-weight: 400;
  }
  .tools { margin-left: auto; }
  .empty-create { margin-top: 12px; }
  .empty { color: var(--text-faint); padding: 40px; text-align: center; }
</style>
