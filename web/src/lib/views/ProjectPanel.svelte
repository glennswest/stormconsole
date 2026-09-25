<script>
  // A project's ownership and fences (#28): who is in it and as what, whether
  // it is isolated, and deleting it. Everything is asked as the viewer, so a
  // project's `view` member sees the members and cannot change them — the
  // apiserver's RBAC answers, and this page shows the answer.
  import { get, call, postJson } from '../api.js'
  import { noteActivity, loadProjects } from '../stores.svelte.js'

  let { name } = $props()

  let data = $state(null)
  let error = $state('')
  let saved = $state('')
  let busy = $state(false)
  let who = $state('')
  let role = $state('edit')
  let dns = $state(true)

  const base = $derived(`/api/plugins/k8s/projects/${encodeURIComponent(name)}`)

  async function load() {
    try {
      data = await get(base)
      error = ''
      dns = data.project.isolated ? data.project.dns : true
    } catch (e) {
      error = e.message
    }
  }
  $effect(() => {
    if (name) load()
  })

  async function run(label, fn) {
    busy = true
    error = ''
    saved = ''
    try {
      const r = await fn()
      saved = r?.message || label
      noteActivity({ reason: label, message: saved, source: `Project/${name}` })
      await load()
    } catch (e) {
      error = e.message
      noteActivity({ reason: label, message: e.message, source: `Project/${name}`, warning: true })
    }
    busy = false
  }

  const addMember = () =>
    run('Add member', async () => {
      const r = await postJson(`${base}/members`, { who, role })
      who = ''
      return r
    })
  const removeMember = (m) =>
    confirm(`Remove ${m.name} (${m.role}) from ${name}?`) &&
    run('Remove member', () => call('DELETE', `${base}/members/${encodeURIComponent(m.binding)}`))
  const isolate = () => run('Isolate', () => postJson(`${base}/isolate`, { dns }))
  const unisolate = () =>
    confirm(`Stop isolating ${name}? Its pods and machines can then be reached from, and reach, anywhere policy allows.`) &&
    run('Remove isolation', () => call('DELETE', `${base}/isolate`))
  async function remove() {
    if (prompt(`Deleting ${name} deletes everything in it — machines, pods, claims. Type its name to confirm.`) !== name) return
    await run('Delete project', () => call('DELETE', base))
    await loadProjects()
    location.hash = '#/projects'
  }
</script>

{#if error}<p class="error">{error}</p>{/if}
{#if saved}<p class="saved">{saved}</p>{/if}

{#if data}
  {@const p = data.project}
  <section class="cards">
    <div class="card">
      <h2>Ownership</h2>
      <dl>
        <dt>Requested by</dt><dd>{p.requester || '—'}</dd>
        <dt>Display name</dt><dd>{p.displayName || '—'}</dd>
        <dt>Description</dt><dd>{p.description || '—'}</dd>
        <dt>Status</dt><dd>{p.phase || '—'}</dd>
      </dl>
      {#if p.system}
        <p class="note">A system namespace: the cluster's own, not somebody's project. Nothing is created here from the console.</p>
      {/if}
    </div>

    <div class="card">
      <h2>Network isolation</h2>
      {#if p.isolated}
        <p><span class="badge">isolated</span> Pods and machines in {name} reach each other and nothing else{p.dns ? ', plus the cluster DNS' : ''}.</p>
      {:else}
        <p class="dim">Not isolated: traffic in and out is whatever other policies allow.</p>
      {/if}
      <p class="dim">Two NetworkPolicies (<span class="mono">storm-isolate</span>, and <span class="mono">storm-isolate-dns</span> for DNS), enforced by Cilium. A VM is covered once it is on the pod network (stormvm#16).</p>
      {#if data.write && !p.system}
        <label class="check"><input type="checkbox" bind:checked={dns} /> allow DNS to the cluster resolver</label>
        <div class="row">
          <button disabled={busy} onclick={isolate}>{p.isolated ? 'Update isolation' : 'Isolate this project'}</button>
          {#if p.isolated}<button disabled={busy} onclick={unisolate}>Remove isolation</button>{/if}
        </div>
      {/if}
    </div>
  </section>

  <section class="card">
    <h2>Members</h2>
    {#if data.membersError}<p class="error">{data.membersError}</p>{/if}
    {#if !data.members.length && !data.membersError}
      <p class="dim">No members bound to admin, edit or view.</p>
    {:else}
      <table class="members">
        <tbody>
          {#each data.members as m (m.binding + m.name)}
            <tr>
              <td>{m.name}</td>
              <td class="dim">{m.kind}{m.namespace ? ` in ${m.namespace}` : ''}</td>
              <td><span class="role {m.role}">{m.role}</span></td>
              <td class="acts">
                {#if data.write}<button class="danger" disabled={busy} onclick={() => removeMember(m)}>Remove</button>{/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
    {#if data.write}
      <div class="row add">
        <input bind:value={who} placeholder="user, or system:serviceaccount:<ns>:<name>" aria-label="Member" />
        <select bind:value={role} aria-label="Role">
          {#each data.roles as r}<option value={r}>{r}</option>{/each}
        </select>
        <button disabled={busy || !who.trim()} onclick={addMember}>Add member</button>
      </div>
      <p class="dim">admin manages the project and its members; edit changes what is in it; view reads it.</p>
    {/if}
  </section>

  {#if data.write && !p.system}
    <section class="card danger-zone">
      <h2>Delete project</h2>
      <p class="dim">Deletes the namespace and everything in it.</p>
      <button class="danger" disabled={busy} onclick={remove}>Delete {name}</button>
    </section>
  {/if}
{:else if !error}
  <p class="dim">Loading…</p>
{/if}

<style>
  .cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 12px; margin-bottom: 12px; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px var(--sc-row-px); margin-bottom: 12px; }
  .cards .card { margin-bottom: 0; }
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0 0 10px; }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 4px 14px; margin: 0; }
  dt { font-size: var(--sc-t-meta); color: var(--text-faint); }
  dd { margin: 0; font-size: var(--sc-t-body); }
  p { font-size: var(--sc-t-body); margin: 0 0 8px; }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .note { color: var(--warn-strong); font-size: var(--sc-t-meta); margin-top: 8px; }
  .mono { font-family: var(--mono); }
  .error { color: var(--error); }
  .saved { color: var(--ok); }
  .badge { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; padding: 1px 6px; border-radius: var(--radius-sm); border: 1px solid var(--accent); color: var(--accent); margin-right: 6px; }
  .row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; margin: 6px 0; }
  .check { display: flex; gap: 6px; align-items: center; font-size: var(--sc-t-meta); margin: 6px 0; }
  .check input { width: auto; }
  .add input { width: 320px; }
  table.members { width: 100%; border-collapse: collapse; margin-bottom: 8px; }
  table.members td { padding: 6px 12px 6px 0; font-size: var(--sc-t-body); }
  table.members tr + tr > td { border-top: 1px solid var(--sc-hairline); }
  .role { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .role.admin { color: var(--accent); }
  .acts { text-align: right; }
  button.danger { color: var(--error); }
  .danger-zone { border-color: var(--error-border, var(--border)); }
</style>
