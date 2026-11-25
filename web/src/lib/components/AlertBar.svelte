<script>
  // A stalled API, said on every page (#123, stormcos#458).
  //
  // The rows are the health plugin's feed rows: an API that is stalled or
  // down is an error whose detail already names the service, the probe and
  // how long, so this bar is a filter over the live feed rather than a
  // second poll — it appears with the push that carries the stall and goes
  // with the one that clears it. Not dismissable: a stall that can be
  // clicked away is the stall nobody noticed on 2026-10-08.
  //
  // A drive's SAS link alarm (#339) is here too: raised from the Drive's
  // LinkDegraded / LinkFailing condition, before a write fails, and kept for
  // an hour after the link is ok again. The drive plugin puts it in the feed
  // as an `alarm` row whose link opens the drive.
  //
  // A RAID set's `BitRot` (#368, stormraid#80) the same way: amber while a
  // member's mismatches or repairs rose in the last scrub pass or day, red
  // with rows past parity (data lost) in the last day, kept an hour after.
  //
  // A drive's own errors (#394) the same way: its node's engine's level
  // (stormblock#648), as stormdrive's DriveErrorsDegraded / DriveErrorsFailing
  // condition on the Drive (stormdrive#160), kept an hour after it is ok.
  import { feed } from '../stores.svelte.js'
  import { linkAlarms } from '../drivelink.js'
  import { bitrotAlarms, errorsAlarms } from '../integrity.js'

  const apis = $derived(
    feed.components.filter((c) => c.kind === 'api' && c.id.startsWith('health:api:') && c.health === 'error'),
  )
  // PID 1's summary itself gone quiet: the node row says so, and no API
  // row can, because they are as of then.
  const node = $derived(feed.components.find((c) => c.id === 'health:node'))
  const apiAlerts = $derived(
    apis.length
      ? apis.map((c) => ({ id: c.id, text: c.detail, href: c.link || '#/health', tone: 'error' }))
      : node?.health === 'error'
        ? [{ id: node.id, text: node.detail, href: '#/health', tone: 'error' }]
        : [],
  )
  const drives = $derived(linkAlarms(feed.components))
  const rot = $derived(bitrotAlarms(feed.components))
  const errs = $derived(errorsAlarms(feed.components))
  // Red first: a stalled API or a failing link before a degraded one.
  const alerts = $derived([...apiAlerts, ...drives, ...errs, ...rot].sort((a, b) => (a.tone === 'error' ? 0 : 1) - (b.tone === 'error' ? 0 : 1)))
  const tone = $derived(alerts.some((a) => a.tone === 'error') ? 'error' : 'warn')
  let open = $state(false)
</script>

{#if alerts.length}
  <div class="alertbar" class:warnbar={tone === 'warn'} role="alert" aria-live="assertive" data-testid="alertbar">
    <span class="mark" aria-hidden="true">{tone === 'warn' ? '!' : '✕'}</span>
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
    {#if apiAlerts.length}<a class="all" href="#/health">API health</a>{/if}
    {#if drives.length}<a class="all" href="#/drives?filter=link">Drive links</a>{/if}
    {#if errs.length}<a class="all" href="#/drives?filter=errors">Drive errors</a>{/if}
    {#if rot.length}<a class="all" href="#/drives/integrity">Integrity</a>{/if}
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
  .alertbar.warnbar {
    background: var(--warn-bg, color-mix(in srgb, var(--warn-strong) 14%, var(--bg)));
    color: var(--warn-strong);
    border-bottom-color: var(--warn-strong);
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
