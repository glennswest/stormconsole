<script>
  // A node's rack (#77): stormipmi's where it can say, an admin's by hand where it cannot.
  import { get, postJson } from '../api.js'
  import { feed } from '../stores.svelte.js'
  import { rackState } from '../rack.js'

  let { host } = $props()
  let info = $state(null)
  let machines = $state(null)
  let editing = $state(false)
  let draft = $state('')
  let saying = $state('')
  let busy = $state(false)

  const base = $derived(`/api/plugins/k8s/nodes/${encodeURIComponent(host)}/rack`)
  // stormipmi says "not started" as idle (#60): then nobody here owns the label.
  const ipmi = $derived(feed.components.find((c) => c.id === 'plugin:ipmi'))
  const answers = $derived(!!ipmi && ipmi.health !== 'idle' && ipmi.health !== 'error')

  async function load() {
    info = await get(base).catch((e) => ({ rack: null, may: false, why: e.message }))
    if (answers) machines = await get('/api/plugins/ipmi/proxy/api/v1/machines').then((d) => d.machines || []).catch(() => null)
    else machines = null
  }
  $effect(() => {
    void answers
    load()
  })
  const s = $derived(rackState(host, info?.rack, machines))

  async function save(rack) {
    busy = true
    try {
      const r = await postJson(base, { rack }, 'PUT')
      saying = r?.error || ''
      if (!r?.error) editing = false
      await load()
    } catch (e) {
      saying = e.message
    }
    busy = false
  }
</script>

{#if info}
  <span class="mono" data-rack>{s.rack || 'none'}</span>
  {#if s.owner === 'stormipmi'}
    <span class="dim">set by stormipmi from where the machine is. <a href={`#/machines`}>Change it on Machines</a></span>
    {#if s.note}<div class="warn small">{s.note}</div>{/if}
  {:else if editing}
    <input type="text" bind:value={draft} aria-label="Rack" placeholder="rack3" disabled={busy} />
    <button class="sc-primary" disabled={busy || !draft.trim()} onclick={() => save(draft.trim())}>Save</button>
    <button disabled={busy} onclick={() => (editing = false)}>Cancel</button>
  {:else if info.may}
    <button class="link" onclick={() => { draft = s.rack; saying = ''; editing = true }}>{s.rack ? 'Change' : 'Set'}</button>
    {#if s.rack}<button class="link" disabled={busy} onclick={() => save('')}>Clear</button>{/if}
  {:else}
    <span class="dim small">{info.why}</span>
  {/if}
  {#if saying}<div class="bad small" role="alert">{saying}</div>{/if}
{/if}

<style>
  .mono { font-family: var(--mono, monospace); }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
  .warn { color: var(--warn-strong); }
  .bad { color: var(--error); }
  input { width: 120px; }
  .link { background: none; border: none; padding: 0 4px; color: var(--accent); cursor: pointer; }
</style>
