<script>
  // Cordon, uncordon and drain this node (#36). Shows what a drain would do before it does anything,
  // and after one what could not be evicted. This is the apiserver's; a cluster member's drain is the Cluster page's.
  import { get, call } from '../api.js'

  let { host, asked = 0 } = $props()
  let plan = $state(null)
  let error = $state('')
  let said = $state('')
  let result = $state(null)
  let busy = $state(false)
  let unmanaged = $state(false)

  const base = $derived(`/api/plugins/k8s/nodes/${encodeURIComponent(host)}`)

  async function load(at) {
    try {
      plan = await get(`${at}/drain`)
      error = ''
    } catch (e) {
      plan = null
      error = e.message
    }
  }
  $effect(() => {
    void asked
    load(base)
  })

  async function act(fn) {
    busy = true
    said = ''
    try {
      const r = await fn()
      said = r?.message || ''
      return r
    } catch (e) {
      if (e.message !== 'cancelled') said = e.message
    } finally {
      await load(base)
      busy = false
    }
  }
  const cordon = (on) => act(() => call('POST', `${base}/${on ? 'cordon' : 'uncordon'}`))

  async function drain() {
    const n = plan.evict.length + (unmanaged ? plan.unmanaged.length : 0)
    if (!confirm(`Cordon ${host} and evict ${n} pod${n === 1 ? '' : 's'}?`)) return
    result = await act(() => call('POST', `${base}/drain`, undefined, { unmanaged }))
  }

  const name = (p) => `${p.namespace}/${p.name}`
  const lost = (list) => list.filter((p) => p.losesData)
</script>

<section class="card" aria-label="Drain">
  <h2>Workloads on this node</h2>
  {#if error}
    <p class="dim">{error}</p>
  {:else if plan}
    <div class="head">
      <span>{plan.cordoned ? 'Cordoned: nothing new is scheduled here.' : 'Schedulable.'}</span>
      {#if plan.may}
        {#if plan.cordoned}
          <button disabled={busy} onclick={() => cordon(false)}>Uncordon</button>
        {:else}
          <button disabled={busy} onclick={() => cordon(true)}>Cordon</button>
        {/if}
        <button class="danger" disabled={busy} onclick={drain}>Drain</button>
      {:else}
        <span class="dim">{plan.why}</span>
      {/if}
    </div>
    {#if plan.warning}<p class="warn">{plan.warning}</p>{/if}
    {#if said}<p class="said" data-testid="said">{said}</p>{/if}

    <dl>
      <dt>A drain evicts</dt>
      <dd data-testid="evict">
        {#each plan.evict as p}<span class="pod mono">{name(p)}</span>{:else}<span class="dim">nothing</span>{/each}
      </dd>
      {#if lost(plan.evict).length}
        <dt>Data lost</dt>
        <dd class="warn">An emptyDir goes with {lost(plan.evict).map(name).join(', ')}.</dd>
      {/if}
      {#if plan.unmanaged.length}
        <dt>No controller</dt>
        <dd data-testid="unmanaged">
          {#each plan.unmanaged as p}<span class="pod mono">{name(p)}</span>{/each}
          <span class="dim">Nothing would bring these back, so a drain leaves them.</span>
          {#if plan.may}
            <label><input type="checkbox" bind:checked={unmanaged} /> Evict these too</label>
          {/if}
        </dd>
      {/if}
      {#if plan.stays.length}
        <dt>Stay</dt>
        <dd data-testid="stays">
          {#each plan.stays as p}<span class="pod mono" title={p.why}>{name(p)}</span>{/each}
        </dd>
      {/if}
      {#if plan.hidden}
        <dt>Not shown</dt>
        <dd class="dim">{plan.hidden} in projects you cannot see</dd>
      {/if}
    </dl>

    {#if result?.blocked?.length}
      <div class="blocked" data-testid="blocked">
        <b>Could not be evicted. Drain again to retry.</b>
        {#each result.blocked as p}<p><span class="mono">{name(p)}</span>: {p.why}</p>{/each}
      </div>
    {/if}
  {/if}
</section>

<style>
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px var(--sc-row-px);
    margin-bottom: 14px;
  }
  h2 {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin: 0 0 10px;
  }
  .head { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; font-size: var(--sc-t-body); }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 6px 14px; margin: 10px 0 0; }
  dt { font-size: var(--sc-t-meta); color: var(--text-faint); }
  dd { margin: 0; font-size: var(--sc-t-meta); min-width: 0; }
  dd label { display: block; margin-top: 4px; }
  .pod { display: inline-block; border: 1px solid var(--border); border-radius: var(--radius); padding: 0 6px; margin: 0 4px 3px 0; overflow-wrap: anywhere; }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); overflow-wrap: anywhere; }
  .warn { color: var(--warn-strong); font-size: var(--sc-t-meta); }
  .said { font-size: var(--sc-t-meta); color: var(--text-dim); margin: 8px 0 0; overflow-wrap: anywhere; }
  .blocked { margin-top: 10px; font-size: var(--sc-t-meta); color: var(--error); overflow-wrap: anywhere; }
  .blocked p { margin: 4px 0 0; }
  button.danger { color: var(--error); }
</style>
