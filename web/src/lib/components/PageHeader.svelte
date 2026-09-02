<script>
  // Every page opens the same way: where you are, what you are looking
  // at, how many of them there are, and what you can do about it. The
  // consistency is the point — it is what separates a console from a
  // collection of screens.
  let {
    crumbs = [],
    title = '',
    scope = '',
    count = null,
    status = null,
    actions = null,
  } = $props()
</script>

{#if crumbs.length}
  <nav class="sc-crumbs" aria-label="Breadcrumb">
    {#each crumbs as c, i}
      {#if i > 0}<span class="sep" aria-hidden="true">/</span>{/if}
      {#if c.href}<a href={c.href}>{c.label}</a>{:else}<span>{c.label}</span>{/if}
    {/each}
  </nav>
{/if}

<div class="sc-pagehead">
  <h1>
    {title}
    {#if scope}<span class="sc-scope">{scope}</span>{/if}
    {#if count !== null}<span class="sc-count">{count}</span>{/if}
    {#if status}{@render status()}{/if}
  </h1>
  {#if actions}<span class="sc-actions">{@render actions()}</span>{/if}
</div>
