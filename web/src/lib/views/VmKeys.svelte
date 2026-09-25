<script>
  // Which SSH keys a machine has, and from where (#26): each Secret its
  // accessCredentials names and how that reaches the guest, and the keys
  // its cloud-init seed carries — which is what actually put them there
  // today, since no node honours accessCredentials yet (stormvm#41).
  import { get, call } from '../api.js'
  import { noteActivity } from '../stores.svelte.js'

  let { ns, name } = $props()

  let data = $state(null)
  let error = $state('')
  let saved = $state('')
  let busy = $state(false)
  const base = $derived(`/api/plugins/vm/vms/${encodeURIComponent(ns)}/${encodeURIComponent(name)}/keys`)

  async function load() {
    try {
      data = await get(base)
      error = ''
    } catch (e) {
      error = e.message
    }
  }
  $effect(() => {
    if (ns && name) load()
  })

  async function addMine() {
    busy = true
    error = ''
    saved = ''
    try {
      const r = await call('POST', base)
      saved = r.message
      noteActivity({ reason: 'Add my keys', message: saved, source: `VirtualMachine/${name}` })
      await load()
    } catch (e) {
      error = e.message
    }
    busy = false
  }

  const HOW = {
    noCloud: 'cloud-init, at first boot',
    configDrive: 'config drive, at first boot',
    qemuGuestAgent: 'guest agent, while running',
  }
</script>

<div class="card">
  <h2>SSH keys</h2>
  {#if error}<p class="error">{error}</p>{/if}
  {#if saved}<p class="saved">{saved}</p>{/if}
  {#if data}
    {#if !data.credentials.length && !data.seed.length}
      <p class="none">No SSH keys. Cloud images have no password, so nothing can log into this machine.</p>
    {/if}
    {#if data.seed.length}
      <div class="src">In its cloud-init seed <span class="dim">(what the guest was given at first boot)</span></div>
      <ul>
        {#each data.seed as k (k.line)}
          <li><span class="mono">{k.short}</span> <span class="dim">{k.comment}</span></li>
        {/each}
      </ul>
    {/if}
    {#each data.credentials as c (c.secret + c.how)}
      <div class="src">
        <span class="mono">{c.secret}</span> <span class="dim">· {HOW[c.how] || c.how}</span>
      </div>
      {#if c.error}<p class="error">{c.error}</p>{/if}
      <ul>
        {#each c.keys as k (k.name)}
          <li><span class="mono">{k.short || k.name}</span> <span class="dim">{k.comment || ''}</span></li>
        {/each}
      </ul>
    {/each}
    {#if data.credentials.length}
      <p class="dim">accessCredentials is KubeVirt's field; no node acts on it yet (stormvm#41), so the seed is what put keys in this guest.</p>
    {/if}
    {#if data.canAdd}
      <button disabled={busy} onclick={addMine} title="Name your saved keys on this machine for the guest agent to write">
        {data.hasMine ? 'Refresh my keys' : 'Add my keys'}
      </button>
    {/if}
  {:else if !error}
    <p class="dim">Loading…</p>
  {/if}
</div>

<style>
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px var(--sc-row-px);
  }
  h2 {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin: 0 0 10px;
  }
  .src { font-size: var(--sc-t-meta); margin-top: 6px; }
  ul { list-style: none; margin: 4px 0 8px; padding: 0; display: grid; gap: 2px; }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .none { margin: 0 0 8px; font-size: var(--sc-t-body); color: var(--warn-strong); }
  .error { margin: 0 0 8px; font-size: var(--sc-t-meta); color: var(--error); }
  .saved { margin: 0 0 8px; font-size: var(--sc-t-meta); color: var(--ok); }
  button { font-size: var(--sc-t-meta); margin-top: 4px; }
</style>
