<script>
  // "+ Create" — the creators a view offers (by its hash route), or every
  // creator when used from the top bar. One button when there is one.
  import { creatorsFor, openCreator } from '../stores.svelte.js'

  let { at = null, label = '+ Create', primary = false } = $props()
  const list = $derived(creatorsFor(at))
  let open = $state(false)

  function pick(c) {
    open = false
    openCreator(c)
  }
</script>

{#if list.length === 1}
  <button class="create" class:primary onclick={() => pick(list[0])}>+ {list[0].label}</button>
{:else if list.length > 1}
  <span class="menu">
    <button class="create" class:primary onclick={() => (open = !open)}>{label} ▾</button>
    {#if open}
      <div class="drop" role="menu">
        {#each list as c (c.id)}
          <button role="menuitem" onclick={() => pick(c)}>
            <span>{c.label}</span>
            <span class="plugin">{c.plugin}</span>
          </button>
        {/each}
      </div>
      <div class="close" onclick={() => (open = false)} role="presentation"></div>
    {/if}
  </span>
{/if}

<style>
  .create {
    padding: 4px 10px; font-size: 12px; border-radius: var(--radius-sm, 4px);
    background: var(--panel-raised); color: var(--text); border: 1px solid var(--border); cursor: pointer;
  }
  .create.primary { background: var(--accent); color: var(--accent-fg, #fff); border-color: var(--accent); font-weight: 600; }
  .menu { position: relative; display: inline-block; }
  .drop {
    position: absolute; right: 0; top: calc(100% + 4px); min-width: 220px; z-index: 40;
    background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius-sm, 4px);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.35); padding: 4px; display: grid;
  }
  .drop button {
    display: flex; justify-content: space-between; gap: 12px; width: 100%; text-align: left;
    background: none; border: none; color: var(--text); padding: 6px 10px; font-size: 13px; cursor: pointer;
    border-radius: var(--radius-sm, 4px);
  }
  .drop button:hover { background: var(--panel-raised); }
  .plugin { color: var(--text-faint); font-size: 11px; }
  .close { position: fixed; inset: 0; z-index: 30; }
</style>
