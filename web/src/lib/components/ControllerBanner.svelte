<script>
  // "HBA mode is required" (#325): one line per RAID controller in RAID mode,
  // and one per controller waiting for the reboot that applies HBA. On every
  // Drives page, because a controller in RAID mode is why its drives look
  // missing. `controllers` given, it shows those; otherwise it reads them.
  import { get } from '../api.js'
  import { banners } from '../controllers.js'

  let { controllers = undefined, link = true } = $props()

  let own = $state(null)
  $effect(() => {
    if (controllers !== undefined) return
    const read = () => get('/api/plugins/drive/controllers').then((v) => (own = v.controllers || [])).catch(() => {})
    read()
    const t = setInterval(read, 15000)
    return () => clearInterval(t)
  })
  const list = $derived(banners(controllers !== undefined ? controllers : own))
</script>

{#each list as b (b.node + b.key + b.code)}
  <div class="banner {b.tone}" role={b.tone === 'warn' ? 'alert' : 'status'} data-testid="controller-banner" data-code={b.code} title={b.detail}>
    <strong>{b.text}</strong>
    {#if link}<a href="#/drives/controllers">Controllers</a>{/if}
  </div>
{/each}

<style>
  .banner { display: flex; flex-wrap: wrap; gap: 4px 12px; align-items: baseline; margin: 0 0 10px; padding: 8px 12px; border-left: 3px solid var(--warn-strong); background: var(--panel-raised); font-size: var(--sc-t-body); overflow-wrap: anywhere; }
  .banner.info { border-left-color: var(--accent); }
  .banner strong { font-weight: 600; flex: 1 1 260px; }
</style>
