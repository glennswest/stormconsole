<script>
  // Admin → Users (#266): the users kept on the apiserver, in sc#27's shape.
  // Every change is made as you, so the apiserver's RBAC decides it.
  import { get, postJson, call } from '../api.js'
  import { ago } from '../ui/time.js'
  import { means, offer, newUserProblem } from '../users.js'

  let data = $state(null)
  let error = $state('')
  let note = $state('')
  let busy = $state(false)
  let fresh = $state({ name: '', password: '', again: '', roles: ['storage-admin'] })
  let editing = $state('')
  let editRoles = $state([])

  const offered = $derived(offer(data?.roles))
  const problem = $derived(newUserProblem(fresh))

  async function load() {
    try {
      data = await get('/api/v1/users')
      error = ''
    } catch (e) {
      error = /^403/.test(e.message) ? "Users is an administrator's." : e.message
    }
  }
  load()

  async function act(what, fn) {
    busy = true
    note = ''
    try {
      await fn()
      note = what
      await load()
    } catch (e) {
      if (e.message !== 'cancelled') note = `${what.replace(/\.$/, '')} failed: ${e.message}`
    } finally {
      busy = false
    }
  }

  function toggle(list, r) {
    return list.includes(r) ? list.filter((x) => x !== r) : [...list, r]
  }

  function create(ev) {
    ev.preventDefault()
    if (problem) return
    const { name, password, roles } = fresh
    act(`User ${name} created.`, async () => {
      await postJson('/api/v1/users', { name, password, roles })
      fresh = { name: '', password: '', again: '', roles: ['storage-admin'] }
    })
  }

  function password(u) {
    const p = window.prompt(`New password for ${u.name}:`)
    if (!p) return
    if (window.prompt(`The same password again, for ${u.name}:`) !== p) {
      note = 'The passwords differ: nothing was changed.'
      return
    }
    act(`Password for ${u.name} set; their other sessions here have ended.`, () =>
      postJson(`/api/v1/users/${u.name}/password`, { password: p }, 'PUT'),
    )
  }

  function disable(u) {
    act(`${u.name} ${u.disabled ? 'enabled' : 'disabled'}.`, () =>
      postJson(`/api/v1/users/${u.name}/disabled`, { disabled: !u.disabled }, 'PUT'),
    )
  }

  function remove(u) {
    // The name, typed: the same habit as every destructive act here.
    const typed = window.prompt(`Delete user ${u.name}, their password and their role bindings?\n\nType ${u.name} to confirm.`)
    if (typed === null) return
    if (typed.trim() !== u.name) {
      note = `Not deleted: that is not ${u.name}.`
      return
    }
    act(`${u.name} deleted.`, () => call('DELETE', `/api/v1/users/${u.name}`))
  }

  function startRoles(u) {
    editing = u.name
    editRoles = [...u.roles]
  }

  function saveRoles(u) {
    act(`Roles of ${u.name} set.`, async () => {
      await postJson(`/api/v1/users/${u.name}/roles`, { roles: editRoles }, 'PUT')
      editing = ''
    })
  }
</script>

<div class="page">
  <nav class="crumbs">Admin / Users</nav>
  <h1>Users</h1>
  <p class="dim small">
    The people who sign in to this cluster's consoles, kept on the apiserver
    {#if data}in <code>{data.namespace}</code>{/if}. A user acts upstream as themselves: their roles are ClusterRoles,
    and every change here is made as you. <code>sc user</code> sees the same users.
  </p>

  {#if error}
    <p class="bad">{error}</p>
  {:else if data}
    {#if note}<p class="note">{note}</p>{/if}
    <table>
      <thead><tr><th>User</th><th>Roles</th><th>Password</th><th>State</th><th>Made</th><th></th></tr></thead>
      <tbody>
        {#each data.users as u (u.name)}
          <tr class:off={u.disabled}>
            <td class="mono">{u.name}{#if u.name === data.you} <span class="dim">(you)</span>{/if}</td>
            <td>
              {#if editing === u.name}
                <div class="roles">
                  {#each offered as r (r)}
                    <label title={means(r)}><input type="checkbox" checked={editRoles.includes(r)} onchange={() => (editRoles = toggle(editRoles, r))} /> {r}</label>
                  {/each}
                </div>
                <button disabled={busy || !editRoles.length} onclick={() => saveRoles(u)}>Save</button>
                <button class="quiet" onclick={() => (editing = '')}>Cancel</button>
              {:else}
                {#each u.roles as r (r)}<span class="role" title={means(r)}>{r}</span>{:else}<span class="dim">none</span>{/each}
              {/if}
            </td>
            <td>{u.password ? 'set' : 'not set'}</td>
            <td class={u.disabled ? 'warn' : 'ok'}>{u.disabled ? 'disabled' : 'enabled'}</td>
            <td class="dim">{u.created ? `${ago(u.created)} ago` : '—'}</td>
            <td class="acts">
              <button disabled={busy} onclick={() => password(u)}>Reset password</button>
              {#if !u.builtin}
                {#if editing !== u.name}<button disabled={busy} onclick={() => startRoles(u)}>Roles</button>{/if}
                {#if u.name !== data.you}
                  <button disabled={busy} onclick={() => disable(u)}>{u.disabled ? 'Enable' : 'Disable'}</button>
                  <button class="danger" disabled={busy} onclick={() => remove(u)}>Delete</button>
                {/if}
              {:else}
                <span class="dim small" title="stormcos's own account, bound to cluster-admin">built in</span>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>

    <h2>New user</h2>
    <form onsubmit={create}>
      <label>Name <input bind:value={fresh.name} autocomplete="off" spellcheck="false" placeholder="e.g. storage1" /></label>
      <label>Password <input type="password" bind:value={fresh.password} autocomplete="new-password" /></label>
      <label>Again <input type="password" bind:value={fresh.again} autocomplete="new-password" /></label>
      <fieldset>
        <legend>Roles</legend>
        {#each offered as r (r)}
          <label title={means(r)}><input type="checkbox" checked={fresh.roles.includes(r)} onchange={() => (fresh.roles = toggle(fresh.roles, r))} /> {r} <span class="dim small">— {means(r)}</span></label>
        {:else}
          <span class="dim">No ClusterRoles could be listed as you.</span>
        {/each}
      </fieldset>
      <div class="row">
        <button type="submit" disabled={busy || !!problem}>Create</button>
        {#if problem && (fresh.name || fresh.password)}<span class="dim small">{problem}</span>{/if}
      </div>
    </form>
  {/if}
</div>

<style>
  .page { padding: 16px var(--sc-gutter, 20px); }
  h1 { margin: 4px 0; }
  h2 { margin: 24px 0 8px; font-size: 1.05em; }
  .crumbs { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .small { font-size: var(--sc-t-meta); }
  .dim { color: var(--text-dim); }
  .mono, code { font-family: var(--mono); }
  .note { color: var(--text); }
  .bad { color: var(--bad, #d55); }
  .warn { color: var(--warn, #c90); }
  .ok { color: var(--ok, #4a4); }
  table { width: 100%; border-collapse: collapse; }
  th { text-align: left; color: var(--text-faint); font-size: var(--sc-t-meta); padding: 4px 8px; }
  td { padding: 6px 8px; border-top: 1px solid var(--sc-hairline, var(--border)); vertical-align: top; }
  tr.off td:first-child { text-decoration: line-through; }
  .role { display: inline-block; margin: 0 4px 2px 0; padding: 0 6px; border: 1px solid var(--border); border-radius: 3px; font-size: var(--sc-t-meta); }
  .roles { display: flex; flex-wrap: wrap; gap: 4px 12px; margin-bottom: 4px; }
  .acts { white-space: nowrap; text-align: right; }
  form { display: flex; flex-direction: column; gap: 8px; max-width: 560px; }
  form > label { display: flex; gap: 8px; align-items: center; color: var(--text-dim); }
  form > label input { flex: 1; }
  fieldset { border: 1px solid var(--border); border-radius: 4px; display: flex; flex-direction: column; gap: 4px; }
  legend { color: var(--text-dim); }
  input:not([type='checkbox']) { background: var(--panel); color: var(--text); border: 1px solid var(--border); border-radius: 4px; padding: 4px 6px; }
  button { background: var(--panel); color: var(--text); border: 1px solid var(--border); border-radius: 4px; padding: 3px 10px; cursor: pointer; margin-left: 4px; }
  button:disabled { opacity: 0.5; cursor: default; }
  button.danger { border-color: var(--bad, #d55); color: var(--bad, #d55); }
  button.quiet { border-color: transparent; }
  .row { display: flex; gap: 8px; align-items: center; }
</style>
