<script>
  // Account → SSH keys (#26): upload a key once, and every machine you
  // create gets it.
  //
  // Kept as a Kubernetes Secret, `<you>-ssh-keys`, one key per item, so
  // the same list is what KubeVirt's accessCredentials names. A machine in
  // another namespace gets a copy there — accessCredentials can only reach
  // a Secret in the machine's own — and editing the list here refreshes
  // every copy.
  import { get, call, postJson } from '../api.js'
  import { noteActivity } from '../stores.svelte.js'
  import PageHeader from '../components/PageHeader.svelte'
  import CopyButton from '../components/CopyButton.svelte'

  let data = $state(null)
  let error = $state('')
  let saved = $state('')
  let busy = $state(false)
  let form = $state({ name: '', key: '' })

  async function load() {
    try {
      data = await get('/api/plugins/vm/keys')
      error = data.error || ''
    } catch (e) {
      error = e.message
    }
  }
  load()

  async function add() {
    busy = true
    error = ''
    saved = ''
    try {
      const r = await postJson('/api/plugins/vm/keys', form)
      saved = r.message
      noteActivity({ reason: 'SSH key', message: saved, source: 'Account' })
      form = { name: '', key: '' }
      await load()
    } catch (e) {
      error = e.message
    }
    busy = false
  }

  async function remove(k) {
    if (!confirm(`Delete key ${k.name}? Machines already running keep it; new ones will not get it.`)) return
    busy = true
    error = ''
    saved = ''
    try {
      const r = await call('DELETE', `/api/plugins/vm/keys/${encodeURIComponent(k.name)}`)
      saved = r.message
      noteActivity({ reason: 'SSH key', message: saved, source: 'Account' })
      await load()
    } catch (e) {
      error = e.message
    }
    busy = false
  }

  // A .pub read in the browser and put in the box, so what is saved is
  // what was seen — nothing is uploaded until Save.
  async function pick(e) {
    const file = e.currentTarget.files?.[0]
    if (!file) return
    form.key = (await file.text()).trim()
    if (!form.name) form.name = file.name.replace(/\.pub$/, '')
    e.currentTarget.value = ''
  }
</script>

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Account' }, { label: 'SSH keys' }]} title="SSH keys" scope={data ? `for ${data.user}` : ''} />

  {#if error}<p class="error">{error}</p>{/if}
  {#if saved}<p class="saved">{saved}</p>{/if}

  <p class="lead">
    Every machine you create gets these keys, for its default user and root, unless you untick them
    on the create form. They are kept as the Secret
    {#if data}<span class="mono">{data.secret}</span> in <span class="mono">{data.home}</span>{/if},
    and a machine in another namespace gets a copy there.
  </p>

  {#if data}
    <section class="card">
      <h2>Saved keys</h2>
      {#if !data.keys.length}
        <p class="none">No saved keys yet. Add one below.</p>
      {:else}
        <table class="keys">
          <tbody>
            {#each data.keys as k (k.name)}
              <tr>
                <td class="mono">{k.name}</td>
                <td class="mono">{k.short || ''}{#if k.error}<span class="bad">{k.error}</span>{/if}</td>
                <td class="dim">{k.comment || ''}</td>
                <td class="acts">
                  <CopyButton value={k.line} label="Copy key" />
                  {#if data.write}<button class="danger" disabled={busy} onclick={() => remove(k)}>Delete</button>{/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
      {#if data.copies?.length}
        <p class="dim">Copies in: {data.copies.join(', ')}</p>
      {/if}
    </section>

    {#if data.config?.length}
      <section class="card">
        <h2>From the console's configuration</h2>
        <p class="dim">Set in the console's config file for {data.user}, and given to new machines too. Change them there.</p>
        <table class="keys">
          <tbody>
            {#each data.config as k (k.name)}
              <tr><td class="mono">{k.name}</td><td class="mono">{k.short || k.line}</td><td class="dim">{k.comment || ''}</td><td></td></tr>
            {/each}
          </tbody>
        </table>
      </section>
    {/if}

    {#if data.write}
      <section class="card">
        <h2>Add a key</h2>
        <div class="add">
          <input bind:value={form.name} placeholder="name (optional — defaults to the key's comment)" aria-label="Key name" />
          <textarea bind:value={form.key} rows="3" spellcheck="false"
            placeholder="ssh-ed25519 AAAA… you@host — paste one key, or a whole authorized_keys file"
            aria-label="Public key"></textarea>
          <div class="row">
            <label class="file">Upload a .pub<input type="file" accept=".pub,text/plain" onchange={pick} /></label>
            <button class="sc-primary" disabled={busy || !form.key.trim()} onclick={add}>Save</button>
          </div>
          <p class="dim">The public half only — the file ending in <span class="mono">.pub</span>. A private key is refused.</p>
        </div>
      </section>
    {/if}
  {/if}
</div>

<style>
  .lead { font-size: var(--sc-t-body); color: var(--text-dim); max-width: 70ch; margin: 0 0 14px; }
  .error { margin: 0 0 12px; font-size: var(--sc-t-body); color: var(--error); }
  .saved { margin: 0 0 12px; font-size: var(--sc-t-body); color: var(--ok); }
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px var(--sc-row-px);
    margin-bottom: 12px;
  }
  h2 {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin: 0 0 10px;
  }
  table.keys { width: 100%; border-collapse: collapse; }
  table.keys td { padding: 6px 12px 6px 0; font-size: var(--sc-t-body); vertical-align: baseline; }
  table.keys tr + tr > td { border-top: 1px solid var(--sc-hairline); }
  .acts { text-align: right; white-space: nowrap; }
  .acts button { font-size: var(--sc-t-meta); padding: 2px 8px; margin-left: 6px; }
  .danger { color: var(--error); }
  .bad { color: var(--error); margin-left: 8px; }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .dim, .none { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .none { font-size: var(--sc-t-body); margin: 0; }
  .add { display: grid; gap: 8px; max-width: 760px; }
  .add textarea { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .row { display: flex; gap: 8px; align-items: center; justify-content: space-between; }
  .file { font-size: var(--sc-t-meta); color: var(--text-dim); cursor: pointer; }
  .file input { margin-left: 8px; font-size: var(--sc-t-meta); }
</style>
