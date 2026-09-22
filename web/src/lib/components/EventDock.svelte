<script>
  // The bottom dock: what is happening, without opening a page for it.
  //
  // vSphere's Recent Tasks and Proxmox's task log both put this at the
  // bottom of every screen, and both are right about why: you press
  // Create, and the question in your head for the next ten seconds is
  // "did that work". Answering it should not cost a navigation — by the
  // time somebody has found the Events page, the thing they wanted to see
  // has either happened or not, and they have lost the thread either way.
  //
  // Two sources, deliberately:
  //
  //  * **what this console did** — appended the instant an action returns,
  //    so a click is acknowledged in the same frame rather than after the
  //    next poll. This is the half that says "your request was sent", and
  //    it is the only half that can, because nothing upstream knows a
  //    button was pressed.
  //  * **what the cluster did about it** — polled. This is the half that
  //    says whether it worked, and it is the one that carries the reason
  //    when it did not.
  //
  // Abbreviated on purpose: one line each, newest first. The full record
  // is the Events page and the object's own box; this is the ticker.
  import { get } from '../api.js'
  import { ago } from '../ui/time.js'
  import { dock, toggleDock, localActivity } from '../stores.svelte.js'

  let cluster = $state([])
  let reason = $state('')
  let timer = null

  async function poll() {
    try {
      const r = await get('/api/v1/console/events/recent')
      cluster = r.available ? r.items : []
      reason = r.available ? '' : r.reason
    } catch (e) {
      reason = e.message
    }
  }

  $effect(() => {
    poll()
    // Five seconds: fast enough that a create feels acknowledged, slow
    // enough that a console left open overnight is not a load generator.
    timer = setInterval(poll, 5000)
    return () => clearInterval(timer)
  })

  // The console's own actions first — they are newer than anything the
  // cluster has had time to say about them, and they are what the person
  // is waiting on.
  const lines = $derived([...localActivity.items, ...cluster].slice(0, 80))
  const warnings = $derived(lines.filter((e) => e.type === 'Warning').length)
</script>

<section class="dock" class:open={dock.open} aria-label="Recent activity">
  <button class="bar" onclick={toggleDock} aria-expanded={dock.open}>
    <span class="caret" class:up={!dock.open}>▾</span>
    <span class="title">Recent activity</span>
    {#if warnings}<span class="warncount">{warnings}</span>{/if}
    {#if !dock.open && lines[0]}
      <!-- Shut, it still says the last thing that happened: a dock that
           hides everything when closed is a dock people leave open. -->
      <span class="peek" class:warn={lines[0].type === 'Warning'}>
        <span class="when">{ago(lines[0].time)}</span>
        <span class="src">{lines[0].source}</span>
        <span class="reason">{lines[0].reason}</span>
        <span class="msg">{lines[0].message}</span>
      </span>
    {/if}
  </button>

  {#if dock.open}
    <div class="feed">
      {#if !lines.length}
        <p class="none">{reason || 'Nothing has happened yet.'}</p>
      {:else}
        <ul>
          {#each lines as e, i (e.id || i)}
            <li class:warn={e.type === 'Warning'} class:mine={e.mine}>
              <span class="when" title={e.time}>{ago(e.time)}</span>
              <span class="src">{e.source}</span>
              <span class="reason">{e.reason}</span>
              <span class="msg">{e.message}</span>
              {#if e.count > 1}<span class="count">×{e.count}</span>{/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</section>

<style>
  .dock {
    grid-area: dock;
    border-top: 1px solid var(--border-strong);
    background: var(--panel);
    display: grid;
    grid-template-rows: auto 1fr;
    min-height: 0;
  }

  .bar {
    display: flex;
    align-items: baseline;
    gap: 9px;
    width: 100%;
    background: color-mix(in srgb, var(--panel-raised) 55%, var(--panel));
    border: none;
    border-radius: 0;
    padding: 5px 12px;
    text-align: left;
    font-size: var(--sc-t-meta);
    color: var(--text-dim);
    min-width: 0;
  }
  .bar:hover { color: var(--text); }
  .caret { transition: transform 0.15s ease; }
  .caret.up { transform: rotate(-90deg); }
  .title {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-weight: 700;
    white-space: nowrap;
  }
  .warncount {
    font-size: var(--sc-t-eyebrow);
    font-weight: 700;
    color: var(--warn-strong);
    background: var(--warn-bg);
    border-radius: 999px;
    padding: 0 6px;
  }
  .peek {
    display: flex;
    gap: 9px;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
  }
  .peek.warn { color: var(--warn-strong); }

  .feed { overflow-y: auto; min-height: 0; padding: 4px 0; }
  ul { list-style: none; margin: 0; padding: 0; }
  li {
    display: grid;
    grid-template-columns: 46px 190px 150px 1fr auto;
    gap: 10px;
    align-items: baseline;
    padding: 3px 12px;
    font-size: var(--sc-t-meta);
    white-space: nowrap;
  }
  li:nth-child(even) { background: var(--sc-zebra); }
  li.warn { background: var(--warn-bg); color: var(--warn-strong); }
  /* What this console did, as opposed to what it observed. */
  li.mine { box-shadow: inset 2px 0 0 var(--accent); }

  .when { font-variant-numeric: tabular-nums; color: var(--text-faint); }
  .src { font-family: var(--mono); color: var(--text-dim); overflow: hidden; text-overflow: ellipsis; }
  .reason { font-weight: 600; overflow: hidden; text-overflow: ellipsis; }
  .msg { color: var(--text-dim); overflow: hidden; text-overflow: ellipsis; }
  li.warn .src, li.warn .msg { color: inherit; }
  .count { font-family: var(--mono); color: var(--text-faint); }
  .none { margin: 0; padding: 6px 12px; font-size: var(--sc-t-meta); color: var(--text-faint); }

  @media (max-width: 900px) {
    li { grid-template-columns: 44px 1fr; }
    li .reason, li .count { display: none; }
  }
</style>
