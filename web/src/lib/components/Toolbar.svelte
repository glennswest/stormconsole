<script>
  // The filter row that sits between a page header and its data: search,
  // whatever filters the view declares, then the view switch and the
  // result count on the right. Mirrors the OpenShift list toolbar.
  import Icon from './Icon.svelte'

  let {
    search = $bindable(''),
    placeholder = 'Search by name',
    onsearch = null,
    filters = null,
    right = null,
    view = $bindable(null),
    onview = null,
    hint = '',
  } = $props()
</script>

<div class="sc-toolbar">
  <label class="sc-search">
    <Icon name="search" size={14} />
    <input
      type="search"
      {placeholder}
      aria-label={placeholder}
      bind:value={search}
      oninput={() => onsearch?.(search)}
    />
  </label>
  {#if filters}{@render filters()}{/if}
  <span class="sc-right">
    {#if hint}<span class="sc-hint">{hint}</span>{/if}
    {#if right}{@render right()}{/if}
    {#if view !== null}
      <span class="sc-seg" role="group" aria-label="View">
        <button
          aria-pressed={view === 'table'}
          title="Table view"
          onclick={() => { view = 'table'; onview?.('table') }}
        ><Icon name="table" size={14} /></button>
        <button
          aria-pressed={view === 'cards'}
          title="Card view"
          onclick={() => { view = 'cards'; onview?.('cards') }}
        ><Icon name="cards" size={14} /></button>
      </span>
    {/if}
  </span>
</div>
