<script>
  // What happened to one object.
  //
  // Asked by component id, so this component knows nothing about kinds and
  // works wherever there is an id — a VM page, an opened row in any table.
  // The plugin that owns the id answers; the host decides which that is.
  //
  // "Nothing happened" and "nothing records events for this" are different
  // answers and are rendered differently. A box that shows them the same
  // way teaches people to distrust it, and a volume with no events is not
  // a volume nothing has happened to.
  import { get } from '../api.js'
  import { ago } from '../ui/time.js'

  let { id, compact = false, refresh = 0 } = $props()

  let state = $state({ loading: true, available: true, reason: '', items: [] })

  async function load() {
    if (!id) return
    try {
      const r = await get(`/api/v1/console/events?id=${encodeURIComponent(id)}`)
      state = { loading: false, ...r }
    } catch (e) {
      state = { loading: false, available: false, reason: e.message, items: [] }
    }
  }

  // `refresh` is bumped by whoever owns this box after an action, because
  // "did my start work" is asked immediately and the answer arrives a
  // second later.
  $effect(() => {
    void id
    void refresh
    load()
  })

  const warnings = $derived(state.items.filter((e) => e.type === 'Warning').length)
</script>

<div class="events" class:compact>
  <div class="head">
    <h3>Events</h3>
    {#if warnings}
      <span class="warncount">{warnings} warning{warnings === 1 ? '' : 's'}</span>
    {/if}
    <button class="reload" onclick={load} aria-label="Reload events">↻</button>
  </div>

  {#if state.loading}
    <p class="none">Reading…</p>
  {:else if !state.available}
    <p class="none">{state.reason}</p>
  {:else if !state.items.length}
    <!-- An answer, not an absence. -->
    <p class="none">Nothing has been recorded about this yet.</p>
  {:else}
    <ul>
      {#each state.items as e, i (i)}
        <li class:warn={e.type === 'Warning'}>
          <span class="when" title={e.time}>{ago(e.time)}</span>
          <span class="reason">{e.reason}</span>
          <span class="msg">{e.message}</span>
          {#if e.count > 1}<span class="count">×{e.count}</span>{/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .events { display: grid; gap: 6px; }
  .head { display: flex; align-items: baseline; gap: 8px; }
  h3 {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    font-weight: 700;
    margin: 0;
  }
  .warncount { font-size: var(--sc-t-meta); color: var(--warn-strong); font-weight: 600; }
  .reload {
    margin-left: auto;
    background: none;
    border: none;
    color: var(--text-faint);
    padding: 0 4px;
    font-size: 13px;
  }
  .reload:hover { color: var(--text); background: none; }

  ul { list-style: none; margin: 0; padding: 0; display: grid; gap: 1px; }
  li {
    display: grid;
    grid-template-columns: 54px 150px 1fr auto;
    gap: 10px;
    align-items: baseline;
    padding: 4px 6px;
    font-size: var(--sc-t-meta);
    border-radius: var(--radius-sm);
  }
  li:nth-child(even) { background: var(--sc-zebra); }
  li.warn { background: var(--warn-bg); color: var(--warn-strong); }
  .when { font-variant-numeric: tabular-nums; color: var(--text-faint); white-space: nowrap; }
  .reason { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .msg { color: var(--text-dim); }
  li.warn .msg { color: inherit; }
  .count { font-family: var(--mono); color: var(--text-faint); }
  .none { margin: 0; font-size: var(--sc-t-meta); color: var(--text-faint); }

  /* In a table row there is less width and less patience. */
  .compact li { grid-template-columns: 48px 130px 1fr auto; padding: 3px 5px; }
  .compact .msg { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
</style>
