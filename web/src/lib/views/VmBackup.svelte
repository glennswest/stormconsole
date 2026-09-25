<script>
  // The Backup tab (#25): a machine's snapshots, and the button that
  // schedules one.
  //
  // The button *schedules*. It creates a `VirtualMachineSnapshot` — the
  // KubeVirt object, so `virtctl` and `oc` see the same thing — and
  // returns; the node idles the filesystems, pauses, clones every disk at
  // one point and resumes, and writes the status. This tab re-reads while
  // anything is in progress, so the row moves on without a reload.
  import { onDestroy } from 'svelte'
  import { get, call, postJson } from '../api.js'
  import { noteActivity } from '../stores.svelte.js'
  import EmptyState from '../components/EmptyState.svelte'

  let { ns, name } = $props()

  const base = $derived(`/api/plugins/vm/vms/${encodeURIComponent(ns)}/${encodeURIComponent(name)}/snapshots`)

  let data = $state(null)
  let error = $state('')
  let saved = $state('')
  let busy = $state('')
  let asking = $state(false)
  let form = $state({ name: '', note: '' })
  let timer = null

  async function load() {
    try {
      data = await get(base)
      error = ''
    } catch (e) {
      error = e.message
    }
    // Quickly while something is moving, slowly otherwise: a scheduled
    // snapshot changes in seconds, a list of finished ones does not.
    const moving = (data?.snapshots || []).some((s) => ['scheduled', 'progress'].includes(s.state)) ||
      (data?.restores || []).some((r) => ['scheduled', 'progress'].includes(r.state))
    clearTimeout(timer)
    timer = setTimeout(load, moving ? 2000 : 10000)
  }

  $effect(() => {
    if (ns && name) load()
  })
  onDestroy(() => clearTimeout(timer))

  async function take() {
    busy = 'take'
    error = ''
    saved = ''
    try {
      const r = await postJson(base, form)
      saved = r.message
      noteActivity({ reason: 'Snapshot', message: saved, source: `VirtualMachine/${name}` })
      asking = false
      form = { name: '', note: '' }
      await load()
    } catch (e) {
      error = e.message
      noteActivity({ reason: 'Snapshot', message: e.message, source: `VirtualMachine/${name}`, warning: true })
    }
    busy = ''
  }

  async function act(label, method, path, question) {
    if (question && !confirm(question)) return
    busy = path
    error = ''
    saved = ''
    try {
      const r = await call(method, path)
      saved = r.message || label
      noteActivity({ reason: label, message: saved, source: `VirtualMachine/${name}` })
      await load()
    } catch (e) {
      error = e.message
      noteActivity({ reason: label, message: e.message, source: `VirtualMachine/${name}`, warning: true })
    }
    busy = ''
  }

  const when = (t) => (t ? new Date(t).toLocaleString() : '—')
</script>

{#if error}<p class="error">{error}</p>{/if}
{#if saved}<p class="saved">{saved}</p>{/if}

{#if !data}
  {#if !error}<p class="dim">Loading snapshots…</p>{/if}
{:else if !data.available}
  <EmptyState icon="volume" title="Snapshots are not available here" hint={data.reason} />
{:else}
  <div class="bar">
    {#if data.write}
      {#if asking}
        <input bind:value={form.name} placeholder={`${name}-<time>`} aria-label="Snapshot name" />
        <input class="note" bind:value={form.note} placeholder="note (optional)" aria-label="Note" />
        <button class="sc-primary" disabled={busy === 'take'} onclick={take}>Take snapshot</button>
        <button disabled={busy === 'take'} onclick={() => (asking = false)}>Cancel</button>
      {:else}
        <button class="sc-primary" onclick={() => (asking = true)}>Snapshot</button>
      {/if}
    {/if}
    <span class="hint">
      Idles the filesystems, pauses the machine, clones every disk at one point, and resumes it.
      The button schedules the snapshot, and it appears below as the node works through it.
    </span>
  </div>

  {#if !data.snapshots.length}
    <p class="none">No snapshots of {name} yet.</p>
  {:else}
    <table class="snaps">
      <thead>
        <tr><th>Snapshot</th><th>Taken</th><th>State</th><th>Disks</th><th>Size</th><th></th></tr>
      </thead>
      <tbody>
        {#each data.snapshots as s (s.name)}
          <tr class:gone={s.deleting}>
            <td>
              <span class="mono">{s.name}</span>
              {#if s.note}<div class="dim">{s.note}</div>{/if}
            </td>
            <td class="mono">{when(s.taken || s.created)}</td>
            <td>
              <span class="badge {s.state}">{s.state}</span>
              <div class="say" class:bad={s.state === 'failed' || s.state === 'waiting'}>{s.say}</div>
              {#if s.indications?.length}<div class="dim">{s.indications.join(' · ')}</div>{/if}
            </td>
            <td class="mono">{s.disks ? s.disks.join(', ') : ''}{#if !s.disks}<span class="dim" title="stormvm#45">not reported</span>{/if}</td>
            <td class="mono">{s.size || ''}{#if !s.size}<span class="dim" title="stormvm#45">not reported</span>{/if}</td>
            <td class="acts">
              {#if data.write}
                <button
                  disabled={!s.ready || !!data.restoreBlocked || !!busy}
                  title={data.restoreBlocked || (s.ready ? `Restore ${name} from ${s.name}` : 'Not ready to restore from')}
                  onclick={() =>
                    act('Restore', 'POST', `${base}/${encodeURIComponent(s.name)}/restore`,
                      `Restore ${name} from ${s.name}? Its disks are replaced with the snapshot's.`)}
                >Restore</button>
                <button
                  class="danger"
                  disabled={!!busy || s.deleting}
                  onclick={() =>
                    act('Delete snapshot', 'DELETE', `${base}/${encodeURIComponent(s.name)}`, `Delete snapshot ${s.name}?`)}
                >Delete</button>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
    {#if data.write && data.restoreBlocked}<p class="dim">Restore: {data.restoreBlocked}.</p>{/if}
  {/if}

  {#if data.restores.length}
    <h2>Restores</h2>
    <table class="snaps">
      <thead><tr><th>Restore</th><th>From</th><th>Asked</th><th>State</th></tr></thead>
      <tbody>
        {#each data.restores as r (r.name)}
          <tr>
            <td class="mono">{r.name}</td>
            <td class="mono">{r.snapshot}</td>
            <td class="mono">{when(r.created)}</td>
            <td><span class="badge {r.state}">{r.state}</span> <span class="say" class:bad={r.state === 'failed'}>{r.say}</span></td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
{/if}

<style>
  .bar { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; margin-bottom: 14px; }
  .bar input { width: 200px; }
  .bar input.note { width: 260px; }
  .hint { flex: 1 1 320px; font-size: var(--sc-t-meta); color: var(--text-faint); }
  .error { margin: 0 0 12px; font-size: var(--sc-t-body); color: var(--error); }
  .saved { margin: 0 0 12px; font-size: var(--sc-t-body); color: var(--ok); }
  .none, .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .none { font-size: var(--sc-t-body); }
  h2 {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin: 20px 0 8px;
  }
  table.snaps { width: 100%; border-collapse: collapse; background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); }
  table.snaps th {
    text-align: left;
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-faint);
    font-weight: 600;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
  }
  table.snaps td { padding: 8px 12px; font-size: var(--sc-t-body); vertical-align: top; }
  table.snaps tr + tr > td { border-top: 1px solid var(--sc-hairline); }
  tr.gone { opacity: 0.5; }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .say { font-size: var(--sc-t-meta); color: var(--text-dim); margin-top: 3px; }
  .say.bad { color: var(--warn-strong); }
  .badge {
    display: inline-block;
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 1px 6px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    color: var(--text-dim);
  }
  .badge.ready { color: var(--ok); border-color: var(--ok); }
  .badge.failed { color: var(--error); border-color: var(--error); }
  .badge.progress, .badge.scheduled { color: var(--accent); border-color: var(--accent); }
  .badge.waiting { color: var(--warn-strong); border-color: var(--warn-strong); }
  .acts { white-space: nowrap; text-align: right; }
  .acts button { font-size: var(--sc-t-meta); padding: 2px 8px; margin-left: 4px; }
  .acts button.danger { color: var(--error); }
</style>
