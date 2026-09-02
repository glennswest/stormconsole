<script>
  // "Create" — the creators a view offers (by its hash route), or every
  // creator when used from the masthead. One button when there is one.
  import { creatorsFor, openCreator } from '../stores.svelte.js'
  import Icon from './Icon.svelte'

  let { at = null, label = 'Create', primary = false } = $props()
  const list = $derived(creatorsFor(at))
  let open = $state(false)

  function pick(c) {
    open = false
    openCreator(c)
  }
</script>

{#if list.length === 1}
  <button class="create" class:sc-primary={primary} onclick={() => pick(list[0])}>
    <Icon name="plus" size={14} stroke={2.2} />
    {list[0].label}
  </button>
{:else if list.length > 1}
  <span class="menu">
    <button
      class="create"
      class:sc-primary={primary}
      aria-haspopup="menu"
      aria-expanded={open}
      onclick={() => (open = !open)}
    >
      <Icon name="plus" size={14} stroke={2.2} />
      {label}
      <span class="caret"><Icon name="down" size={12} stroke={2.2} /></span>
    </button>
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
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 12px;
    font-size: var(--sc-t-meta);
  }
  .caret { display: inline-grid; place-items: center; opacity: 0.8; margin-left: -1px; }
  .menu { position: relative; display: inline-block; }
  .drop {
    position: absolute;
    right: 0;
    top: calc(100% + 5px);
    min-width: 240px;
    z-index: 40;
    background: var(--panel);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.32);
    padding: 4px;
    display: grid;
  }
  .drop button {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 14px;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    color: var(--text);
    padding: 7px 10px;
    font-size: var(--sc-t-body);
    font-weight: 400;
    border-radius: var(--radius-sm);
  }
  .drop button:hover { background: var(--nav-active); }
  .plugin {
    color: var(--text-faint);
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .close { position: fixed; inset: 0; z-index: 30; }
</style>
