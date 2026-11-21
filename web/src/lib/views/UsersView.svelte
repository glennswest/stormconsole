<script>
  // Admin → Users (#266): the users kept on the apiserver, in sc#27's shape.
  // Every change is made as you, so the apiserver's RBAC decides it.
  // Groups (#334): a group's roles are bindings naming each member, so a
  // member holds them upstream exactly as their own.
  import { get, postJson, call } from '../api.js'
  import { ago } from '../ui/time.js'
  import { means, offer, newUserProblem, groupNameProblem, viaGroups, memberChoices } from '../users.js'

  let data = $state(null)
  let error = $state('')
  let note = $state('')
  let busy = $state(false)
  let fresh = $state({ name: '', password: '', again: '', roles: ['storage-admin'], groups: [] })
  let editing = $state('')
  let editRoles = $state([])
  const blankGroup = () => ({ name: '', description: '', members: [], roles: ['storage-admin'] })
  let freshGroup = $state(blankGroup())
  // Which group is being edited, and what: 'members' or 'roles'.
  let gEditing = $state({ name: '', what: '', list: [] })

  const offered = $derived(offer(data?.roles))
  const problem = $derived(newUserProblem(fresh))
  const groupProblem = $derived(groupNameProblem(freshGroup.name))
  const choices = $derived(memberChoices(data?.users))

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
    const { name, password, roles, groups } = fresh
    act(`User ${name} created.`, async () => {
      await postJson('/api/v1/users', { name, password, roles, groups })
      fresh = { name: '', password: '', again: '', roles: ['storage-admin'], groups: [] }
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
    const typed = window.prompt(`Delete user ${u.name}, their password and their role bindings${u.groups?.length ? `, and take them out of ${u.groups.join(', ')}` : ''}?\n\nType ${u.name} to confirm.`)
    if (typed === null) return
    if (typed.trim() !== u.name) {
      note = `Not deleted: that is not ${u.name}.`
      return
    }
    act(`${u.name} deleted.`, () => call('DELETE', `/api/v1/users/${u.name}`))
  }

  function startRoles(u) {
    editing = u.name
    // Their own: what comes from a group is changed on the group.
    editRoles = [...(u.own || u.roles)]
  }

  function createGroup(ev) {
    ev.preventDefault()
    if (groupProblem) return
    const g = { ...freshGroup }
    act(`Group ${g.name} created.`, async () => {
      await postJson('/api/v1/groups', g)
      freshGroup = blankGroup()
    })
  }

  function startGroup(g, what) {
    gEditing = { name: g.name, what, list: [...(what === 'members' ? g.members : g.roles)] }
  }

  function saveGroup(g) {
    const { what, list } = gEditing
    act(`${what === 'members' ? 'Members' : 'Roles'} of ${g.name} set.`, async () => {
      await postJson(`/api/v1/groups/${g.name}/${what}`, { [what]: list }, 'PUT')
      gEditing = { name: '', what: '', list: [] }
    })
  }

  function removeGroup(g) {
    const typed = window.prompt(`Delete group ${g.name}? Its ${g.members.length} member(s) lose the roles it gives (${g.roles.join(', ') || 'none'}); the users stay.\n\nType ${g.name} to confirm.`)
    if (typed === null) return
    if (typed.trim() !== g.name) {
      note = `Not deleted: that is not ${g.name}.`
      return
    }
    act(`Group ${g.name} deleted.`, () => call('DELETE', `/api/v1/groups/${g.name}`))
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
    and every change here is made as you. A group's roles are held by each member as their own. <code>sc user</code> sees the same users.
  </p>

  {#if error}
    <p class="bad">{error}</p>
  {:else if data}
    {#if note}<p class="note">{note}</p>{/if}
    <table>
      <thead><tr><th>User</th><th>Roles</th><th>Groups</th><th>Password</th><th>State</th><th>Made</th><th></th></tr></thead>
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
                <button disabled={busy || (!editRoles.length && !u.groups?.length)} onclick={() => saveRoles(u)}>Save</button>
                <button class="quiet" onclick={() => (editing = '')}>Cancel</button>
              {:else}
                {#each u.own || u.roles as r (r)}<span class="role" title={means(r)}>{r}</span>{/each}
                {#each viaGroups(u, data.groups) as v (v.role)}<span class="role via" title={`${means(v.role)}, from ${v.from.join(', ')}`}>{v.role} <span class="dim">· {v.from.join(', ')}</span></span>{/each}
                {#if !u.roles.length}<span class="dim">none</span>{/if}
              {/if}
            </td>
            <td>{#each u.groups || [] as g (g)}<span class="role">{g}</span>{:else}<span class="dim">—</span>{/each}</td>
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
      {#if data.groups?.length}
        <fieldset>
          <legend>Groups</legend>
          {#each data.groups as g (g.name)}
            <label><input type="checkbox" checked={fresh.groups.includes(g.name)} onchange={() => (fresh.groups = toggle(fresh.groups, g.name))} /> {g.name} <span class="dim small">— {g.roles.join(', ') || 'no roles'}</span></label>
          {/each}
        </fieldset>
      {/if}
      <div class="row">
        <button type="submit" disabled={busy || !!problem}>Create</button>
        {#if problem && (fresh.name || fresh.password)}<span class="dim small">{problem}</span>{/if}
      </div>
    </form>

    <h2>Groups</h2>
    <p class="dim small">Give a team a role once: every member holds the group's roles upstream, as their own. Kept as ConfigMaps <code>&lt;name&gt;-group</code> and bindings <code>storm-group:&lt;group&gt;:&lt;role&gt;</code>.</p>
    <table aria-label="Groups">
      <thead><tr><th>Group</th><th>Members</th><th>Roles</th><th>Made</th><th></th></tr></thead>
      <tbody>
        {#each data.groups || [] as g (g.name)}
          <tr>
            <td><span class="mono">{g.name}</span>{#if g.description}<div class="dim small">{g.description}</div>{/if}</td>
            <td>
              {#if gEditing.name === g.name && gEditing.what === 'members'}
                <div class="roles">
                  {#each choices as m (m)}
                    <label><input type="checkbox" checked={gEditing.list.includes(m)} onchange={() => (gEditing.list = toggle(gEditing.list, m))} /> {m}</label>
                  {/each}
                </div>
                <button disabled={busy} onclick={() => saveGroup(g)}>Save</button>
                <button class="quiet" onclick={() => (gEditing = { name: '', what: '', list: [] })}>Cancel</button>
              {:else}
                {#each g.members as m (m)}<span class="role mono">{m}</span>{:else}<span class="dim">nobody</span>{/each}
              {/if}
            </td>
            <td>
              {#if gEditing.name === g.name && gEditing.what === 'roles'}
                <div class="roles">
                  {#each offered as r (r)}
                    <label title={means(r)}><input type="checkbox" checked={gEditing.list.includes(r)} onchange={() => (gEditing.list = toggle(gEditing.list, r))} /> {r}</label>
                  {/each}
                </div>
                <button disabled={busy} onclick={() => saveGroup(g)}>Save</button>
                <button class="quiet" onclick={() => (gEditing = { name: '', what: '', list: [] })}>Cancel</button>
              {:else}
                {#each g.roles as r (r)}<span class="role" title={means(r)}>{r}</span>{:else}<span class="dim">none</span>{/each}
              {/if}
            </td>
            <td class="dim">{g.created ? `${ago(g.created)} ago` : '—'}</td>
            <td class="acts">
              {#if gEditing.name !== g.name}
                <button disabled={busy} onclick={() => startGroup(g, 'members')}>Members</button>
                <button disabled={busy} onclick={() => startGroup(g, 'roles')}>Roles</button>
              {/if}
              <button class="danger" disabled={busy} onclick={() => removeGroup(g)}>Delete</button>
            </td>
          </tr>
        {:else}
          <tr><td colspan="5" class="dim">No groups yet.</td></tr>
        {/each}
      </tbody>
    </table>

    <h2>New group</h2>
    <form onsubmit={createGroup} aria-label="New group">
      <label>Name <input bind:value={freshGroup.name} autocomplete="off" spellcheck="false" placeholder="e.g. drives" /></label>
      <label>Description <input bind:value={freshGroup.description} autocomplete="off" placeholder="e.g. replaces and formats drives" /></label>
      <fieldset>
        <legend>Members</legend>
        {#each choices as m (m)}
          <label><input type="checkbox" checked={freshGroup.members.includes(m)} onchange={() => (freshGroup.members = toggle(freshGroup.members, m))} /> {m}</label>
        {:else}
          <span class="dim">No users to add yet: admin is in no group.</span>
        {/each}
      </fieldset>
      <fieldset>
        <legend>Roles</legend>
        {#each offered as r (r)}
          <label title={means(r)}><input type="checkbox" checked={freshGroup.roles.includes(r)} onchange={() => (freshGroup.roles = toggle(freshGroup.roles, r))} /> {r} <span class="dim small">— {means(r)}</span></label>
        {/each}
      </fieldset>
      <div class="row">
        <button type="submit" disabled={busy || !!groupProblem}>Create group</button>
        {#if groupProblem && freshGroup.name}<span class="dim small">{groupProblem}</span>{/if}
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
  .role.via { border-style: dashed; }
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
