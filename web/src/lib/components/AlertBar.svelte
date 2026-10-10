<script>
  // A stalled API, said on every page (#123, stormcos#458).
  //
  // The rows are the health plugin's feed rows: an API that is stalled or
  // down is an error whose detail already names the service, the probe and
  // how long, so this bar is a filter over the live feed rather than a
  // second poll — it appears with the push that carries the stall and goes
  // with the one that clears it. Not dismissable: a stall that can be
  // clicked away is the stall nobody noticed on 2026-10-08.
  import { feed } from '../stores.svelte.js'

  const apis = $derived(
    feed.components.filter((c) => c.kind === 'api' && c.id.startsWith('health:api:') && c.health === 'error'),
  )
  // PID 1's summary itself gone quiet: the node row says so, and no API
  // row can, because they are as of then.
  const node = $derived(feed.components.find((c) => c.id === 'health:node'))
  const alerts = $derived(
    apis.length
      ? apis.map((c) => ({ id: c.id, text: c.detail, href: c.link || '#/health' }))
      : node?.health === 'error'
        ? [{ id: node.id, text: node.detail, href: '#/health' }]
        : [],
  )
  let open = $state(false)
</script>

{#if alerts.length}
  <div class="alertbar" role="alert" aria-live="assertive">
    <span class="mark" aria-hidden="true">✕</span>
    <div class="body">
      <a href={alerts[0].href}>{alerts[0].text}</a>
      {#if alerts.length > 1}
        <button class="more" onclick={() => (open = !open)} aria-expanded={open}>
          {open ? 'fewer' : `and ${alerts.length - 1} more`}
        </button>
        {#if open}
          <ul>
            {#each alerts.slice(1) as a (a.id)}<li><a href={a.href}>{a.text}</a></li>{/each}
          </ul>
        {/if}
      {/if}
    </div>
    <a class="all" href="#/health">API health</a>
  </div>
{/if}

<style>
  .alertbar {
    position: sticky;
    top: 0;
    z-index: 5;
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 8px var(--sc-gutter, 16px);
    background: var(--error-bg);
    color: var(--error);
    border-bottom: 1px solid var(--error);
    font-size: var(--sc-t-body);
  }
  .mark {
    display: inline-grid;
    place-items: center;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 1px solid currentColor;
    font-size: 10px;
    font-weight: 700;
    flex-shrink: 0;
    margin-top: 2px;
  }
  .body {
    flex: 1;
    min-width: 0;
  }
  a {
    color: inherit;
    font-weight: 600;
  }
  .all {
    font-weight: 400;
    white-space: nowrap;
  }
  .more {
    background: none;
    border: 0;
    padding: 0 0 0 8px;
    color: inherit;
    text-decoration: underline;
    cursor: pointer;
    font: inherit;
  }
  ul {
    margin: 4px 0 0;
    padding-left: 18px;
  }
</style>
