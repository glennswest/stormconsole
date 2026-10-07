<script>
  // Cluster (#63, #88): what the cluster is made of, from stormcluster's
  // feed — the cluster (or this node as a single-node cluster), its members
  // and their roles, the nodes discovered that are not members, and the
  // last operations with their steps.
  //
  // A change is a cluster.storm.io object (stormcluster#12): a `Cluster`
  // forms the cluster seeded on this node, a `ClusterMember` per node joins,
  // promotes, demotes, drains or serves storage, and deleting one releases
  // the node (its data erased) or dissolves the cluster. The plugin writes
  // them as the viewer, so the apiserver's RBAC decides. Before anything is
  // written the page asks stormcluster for the plan and shows its steps in
  // stormcluster's own words; a refusal is every reason it gave.
  //
  // What stormcluster made of each object is its status: the phase, the
  // blockers (refusals are status, planned again on every pass), and the
  // operation's step or error. A failed operation resumes by itself, so
  // there is no Resume here.
  import { onDestroy } from 'svelte'
  import { get, postJson, call } from '../api.js'
  import { noteActivity } from '../stores.svelte.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import CopyButton from '../components/CopyButton.svelte'

  const BASE = '/api/plugins/cluster'
  const API = `${BASE}/proxy/api/v1`

  let cards = $state(null)
  let objs = $state({ installed: true, reason: '', clusters: [], members: [] })
  let me = $state({ write: false, why: '' })
  let self = $state('')
  let error = $state('')
  let timer = null

  // The answer of the last thing done: a message, or the reasons.
  let outcome = $state(null)
  let busy = $state(false)
  // An open plan, waiting for Write or Cancel.
  let pending = $state(null)
  let typed = $state('')
  // Steps of each operation that has been opened, by id.
  let opened = $state({})

  async function load() {
    try {
      cards = await get(`${API}/components`)
      error = ''
      for (const id of Object.keys(opened)) readOp(id)
    } catch (e) {
      error = `stormcluster did not answer: ${e.message}. It runs on every node at :9102; is it running on this one, or is [stormcluster] url pointing somewhere else?`
    }
    try {
      objs = await get(`${BASE}/objects`)
    } catch (e) {
      objs = { installed: false, reason: e.message, clusters: [], members: [] }
    }
    clearTimeout(timer)
    timer = setTimeout(load, 3000)
  }
  load()
  get(`${BASE}/me`).then((d) => (me = d)).catch(() => {})
  get(`${API}/self`).then((d) => (self = d.node || '')).catch(() => {})
  onDestroy(() => clearTimeout(timer))

  const metric = (c, label) => c?.metrics?.find((m) => m.label === label)?.value ?? ''
  const opId = (c) => c.id.replace(/^op:/, '')

  const system = $derived(cards?.find((c) => c.id === 'system'))
  const members = $derived((cards || []).filter((c) => c.kind === 'member'))
  const peers = $derived((cards || []).filter((c) => c.kind === 'peer'))
  const ops = $derived((cards || []).filter((c) => c.kind === 'operation'))
  const inCluster = $derived(members.length > 0)
  const masters = $derived(members.filter((m) => metric(m, 'role') === 'master'))
  const workers = $derived(members.filter((m) => metric(m, 'role') === 'worker'))
  // The nodes a form or a join can take: SNOs stormcluster calls available
  // (not stale, stormcluster running there, not a laptop), and not this one.
  const available = $derived(peers.filter((p) => p.detail === 'single-node cluster, available' && p.label !== self))
  const cluster = $derived(objs.clusters[0] || null)
  const memberObj = (node) => objs.members.find((o) => o.name === node)
  // Objects for nodes that are not members yet: joins in progress, or
  // blocked, and what the form wrote before its Cluster formed.
  const requested = $derived(objs.members.filter((o) => !members.some((m) => m.label === o.name)))
  const busyCluster = $derived(!!cluster && !['Ready', 'Paused'].includes(cluster.status?.phase || 'Ready'))

  // --- the plan, then the write ------------------------------------------
  function done(title, message) {
    outcome = { title, message }
    noteActivity({ reason: title, message, source: 'Cluster' })
  }
  function refused(title, data, status) {
    pending = null
    const reasons = data?.refused?.length ? data.refused : [data?.error || `answered ${status}`]
    outcome = { title, reasons, coordinator: data?.coordinator, bad: true }
    noteActivity({ reason: title, message: reasons.join('; '), source: 'Cluster', warning: true })
  }

  /// Ask stormcluster for the plan of `request`, and open it. `write` is
  /// what the confirm does; `word`, when set, must be typed first.
  async function preview(title, request, write, { word = '', warning = '' } = {}) {
    busy = true
    outcome = null
    typed = ''
    try {
      if (!request) {
        pending = { title, plan: null, write, word, warning }
        return
      }
      const r = await fetch(`${BASE}/plan`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(request),
      })
      const data = await r.json().catch(() => ({}))
      if (!r.ok) return refused(title, data, r.status)
      pending = { title, plan: data.plan || { steps: [], warnings: [] }, coordinator: data.coordinator, write, word, warning }
    } catch (e) {
      refused(title, { error: e.message }, 0)
    } finally {
      busy = false
    }
  }

  async function confirmPending() {
    const { title, write } = pending
    busy = true
    pending = null
    try {
      const r = await write()
      done(title, r?.message || 'written')
      form.open = join.open = promote.open = false
      await load()
    } catch (e) {
      refused(title, { error: e.message }, 0)
    } finally {
      busy = false
    }
  }

  const patchMember = (node, change) =>
    postJson(`${BASE}/members/${encodeURIComponent(node)}`, change, 'PATCH')

  async function patchAll(nodes, change) {
    const said = []
    for (const n of nodes) said.push((await patchMember(n, change)).message)
    return { message: said.join(' · ') }
  }

  // --- a member's own changes -------------------------------------------
  function demote(m) {
    preview(`Demote ${m.label}`, { op: 'demote', node: m.label }, () => patchMember(m.label, { role: 'worker' }))
  }
  function drain(m, on) {
    const t = `${on ? 'Drain' : 'Uncordon'} ${m.label}`
    preview(t, { op: on ? 'drain' : 'uncordon', node: m.label }, () => patchMember(m.label, { drain: on }))
  }
  function storage(m, on) {
    const t = `${on ? 'Serve storage from' : 'Stop serving storage from'} ${m.label}`
    preview(t, { op: 'storage', node: m.label, storage: on }, () => patchMember(m.label, { storage: on }))
  }
  function release(node) {
    preview(`Release ${node}`, { op: 'split', node }, () => call('DELETE', `${BASE}/members/${encodeURIComponent(node)}`), {
      word: node,
      warning: `${node} is drained, its data is erased, and it becomes a new single-node cluster. This is not undone.`,
    })
  }
  function dissolve() {
    const name = cluster.name
    preview(`Dissolve ${name}`, null, () => call('DELETE', `${BASE}/clusters/${encodeURIComponent(name)}`), {
      word: name,
      warning: `Every member is released — workers first, the seed last — each drained, its data erased, a new single-node cluster. This is not undone.`,
    })
  }
  const drained = (m) => (memberObj(m.label)?.status?.drained ?? /drained|cordoned/i.test(m.detail)) === true
  const serving = (m) => !!memberObj(m.label)?.spec?.storage

  async function readOp(id) {
    try {
      opened[id] = await get(`${API}/operations/${encodeURIComponent(id)}`)
    } catch (e) {
      opened[id] = { error: e.message }
    }
  }
  function toggleOp(c) {
    const id = opId(c)
    if (opened[id]) delete opened[id]
    else readOp(id)
  }

  // --- Form a cluster ---------------------------------------------------
  // Seeded on this node: a Cluster on its apiserver forms there.
  let form = $state({ open: false, name: '', masters: [], workers: [] })
  function openForm() {
    form = { open: true, name: 'storm', masters: self ? [self] : [], workers: [] }
  }
  function toggle(list, node) {
    const i = list.indexOf(node)
    if (i >= 0) list.splice(i, 1)
    else list.push(node)
  }
  function pickMaster(node) {
    if (node === self) return
    toggle(form.masters, node)
    form.workers = form.workers.filter((w) => w !== node)
  }
  function pickWorker(node) {
    toggle(form.workers, node)
    form.masters = form.masters.filter((m) => m !== node)
  }
  const formWhy = $derived(
    !self ? 'waiting for stormcluster to say which node this is'
    : !form.name.trim() ? 'name the cluster'
    : ![1, 3, 5].includes(form.masters.length) ? `${form.masters.length} master(s): the control plane must be 1, 3 or 5`
    : ''
  )
  function submitForm() {
    const body = { name: form.name.trim(), masters: [...form.masters], workers: [...form.workers] }
    preview(`Form ${body.name}`, { op: 'form', ...body }, () => postJson(`${BASE}/form`, body))
  }

  // --- Join, and promote in pairs ---------------------------------------
  let join = $state({ open: false, role: 'worker', nodes: [] })
  let promote = $state({ open: false, nodes: [] })
  const joinWhy = $derived(
    !join.nodes.length ? 'choose the nodes to join'
    : join.role === 'master' && (masters.length + join.nodes.length) % 2 === 0
      ? `${masters.length} + ${join.nodes.length} masters is even: join masters in pairs`
    : ''
  )
  const promoteWhy = $derived(
    !promote.nodes.length ? 'choose the workers to promote'
    : (masters.length + promote.nodes.length) % 2 === 0
      ? `${masters.length} + ${promote.nodes.length} masters is even: promote in pairs`
    : ''
  )
  function submitJoin() {
    const body = { nodes: [...join.nodes], role: join.role }
    preview(`Join ${body.nodes.join(', ')} as ${body.role}s`, { op: 'join', ...body }, () => postJson(`${BASE}/members`, body))
  }
  function submitPromote() {
    const nodes = [...promote.nodes]
    preview(`Promote ${nodes.join(', ')}`, { op: 'promote', nodes }, () => patchAll(nodes, { role: 'master' }))
  }

  const hardware = (c) => ['cores', 'memory', 'drives'].map((k) => metric(c, k) && `${metric(c, k)} ${k === 'memory' ? '' : k}`.trim()).filter(Boolean).join(' · ')
  const when = (t) => (t ? new Date(t).toLocaleString() : '')
</script>

{#snippet objstatus(o)}
  {#if o}
    {@const st = o.status || {}}
    <div class="objst">
      <span class="phase ph-{(st.phase || 'Pending').toLowerCase()}">{o.deleting ? 'Releasing' : st.phase || 'Pending'}</span>
      {#if st.message}<span class="dim">{st.message}</span>{/if}
      {#if st.operation}
        <div class="op">
          <span class="mono">{st.operation.id}</span> · {st.operation.state}{#if st.operation.step} · {st.operation.step}{/if}
          {#if st.operation.error}<div class="bad">{st.operation.error}</div>{/if}
        </div>
      {/if}
      {#if st.blockers?.length}
        <ul class="reasons blockers">{#each st.blockers as b}<li>{b}</li>{/each}</ul>
      {/if}
      {#if st.suggestedName}<span class="dim">suggested name {st.suggestedName}</span>{/if}
    </div>
  {/if}
{/snippet}

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Cluster' }, { label: 'Membership' }]} title={system?.label || 'Cluster'} count={cards ? members.length || null : null} />

  {#if !me.write && cards}<p class="dim">{me.why}</p>{/if}
  {#if error}<p class="error">{error}</p>{/if}
  {#if !objs.installed}<p class="warn">{objs.reason}</p>{/if}

  {#if outcome}
    <section class="outcome" class:bad={outcome.bad} role="status">
      <strong>{outcome.title}</strong>
      {#if outcome.bad}
        <span>{outcome.reasons.length > 1 ? 'refused, for these reasons:' : 'refused:'}</span>
        <ul class="reasons">{#each outcome.reasons as r}<li>{r}</li>{/each}</ul>
        {#if outcome.coordinator}<div class="dim">answered by {outcome.coordinator}, which coordinates this</div>{/if}
      {:else}
        <span>{outcome.message}</span>
      {/if}
      <button class="link" onclick={() => (outcome = null)}>dismiss</button>
    </section>
  {/if}

  {#if pending}
    <div class="scrim" role="presentation" onclick={() => (pending = null)}></div>
    <div class="dialog" role="dialog" aria-modal="true" aria-label="The plan">
      <h2>{pending.title}</h2>
      {#if pending.plan}
        <p class="dim">
          Nothing is written yet. This is the plan stormcluster made{pending.coordinator ? ` on ${pending.coordinator}, which coordinates it` : ''}:
        </p>
        <ol class="steps">
          {#each pending.plan.steps as s}<li>{s.description}</li>{/each}
        </ol>
        {#if pending.plan.names && Object.keys(pending.plan.names).length}
          <p class="dim">Suggested names (nothing is renamed): {Object.entries(pending.plan.names).map(([n, s]) => `${n} → ${s}`).join(', ')}</p>
        {/if}
        {#if pending.plan.warnings?.length}
          <ul class="warnings">{#each pending.plan.warnings as w}<li>{w}</li>{/each}</ul>
        {/if}
      {/if}
      {#if pending.warning}<p class="bad">{pending.warning}</p>{/if}
      {#if pending.word}
        <label class="field">Type <span class="mono">{pending.word}</span> to confirm <input bind:value={typed} aria-label="Confirm by typing {pending.word}" /></label>
      {/if}
      <div class="dbar">
        <button class="sc-primary" class:danger={!!pending.word} disabled={busy || (pending.word && typed !== pending.word)} onclick={confirmPending}>Write it</button>
        <button disabled={busy} onclick={() => (pending = null)}>Cancel</button>
      </div>
    </div>
  {/if}

  {#if system}
    <section class="card summary health-{system.health}">
      <div>
        <span class="dot {system.health}" aria-hidden="true"></span>
        <strong>{system.label}</strong>
        <span class="dim">{system.detail}</span>
      </div>
      <div class="metrics">
        {#each system.metrics || [] as m}<span><span class="dim">{m.label}</span> <span class="mono">{m.value}</span></span>{/each}
      </div>
      {#if cluster}
        <div><span class="dim">Cluster object</span> <span class="mono">{cluster.name}</span> {@render objstatus(cluster)}</div>
      {/if}
      {#if me.write && objs.installed}
        <div class="forms">
          {#if !inCluster && !cluster}
            <button disabled={busy || !available.length || !self} onclick={openForm}>Form a cluster…</button>
          {:else}
            <button disabled={busy || busyCluster || !available.length} onclick={() => (join = { open: true, role: 'worker', nodes: [] })}>Join nodes…</button>
            <button disabled={busy || busyCluster || !workers.length} onclick={() => (promote = { open: true, nodes: [] })}>Promote workers…</button>
            {#if cluster && !cluster.deleting}<button class="danger" disabled={busy} onclick={dissolve}>Dissolve…</button>{/if}
          {/if}
        </div>
      {/if}
    </section>
  {/if}

  {#if form.open}
    <section class="card">
      <h2>Form a cluster</h2>
      <p class="dim">Seeded on {self}, the node this console runs on: its CA, fastetcd and data become the cluster's. The control plane is 1, 3 or 5 masters.</p>
      <label class="field">Name <input bind:value={form.name} aria-label="Cluster name" /></label>
      <table class="pick">
        <thead><tr><th>Node</th><th>Master</th><th>Worker</th><th></th></tr></thead>
        <tbody>
          <tr>
            <td class="mono">{self} <span class="tagchip">seed</span></td>
            <td><input type="checkbox" aria-label="{self} as master" checked disabled /></td>
            <td></td>
            <td></td>
          </tr>
          {#each available as p (p.id)}
            <tr>
              <td class="mono">{p.label}{#if metric(p, 'suggested name')} <span class="dim">({metric(p, 'suggested name')})</span>{/if}</td>
              <td><input type="checkbox" aria-label="{p.label} as master" checked={form.masters.includes(p.label)} onchange={() => pickMaster(p.label)} /></td>
              <td><input type="checkbox" aria-label="{p.label} as worker" checked={form.workers.includes(p.label)} onchange={() => pickWorker(p.label)} /></td>
              <td class="dim">{hardware(p)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="dbar">
        <button class="sc-primary" disabled={busy || !!formWhy} onclick={submitForm}>Show the plan</button>
        <button onclick={() => (form.open = false)}>Cancel</button>
        {#if formWhy}<span class="dim">{formWhy}</span>{/if}
      </div>
    </section>
  {/if}

  {#if join.open}
    <section class="card">
      <h2>Join nodes</h2>
      <div class="keep">
        <label><input type="radio" name="role" checked={join.role === 'worker'} onchange={() => (join.role = 'worker')} /> as workers</label>
        <label><input type="radio" name="role" checked={join.role === 'master'} onchange={() => (join.role = 'master')} /> as masters (in pairs, so {masters.length} stays odd)</label>
      </div>
      <table class="pick">
        <tbody>
          {#each available as p (p.id)}
            <tr>
              <td><input type="checkbox" aria-label="Join {p.label}" checked={join.nodes.includes(p.label)} onchange={() => toggle(join.nodes, p.label)} /></td>
              <td class="mono">{p.label}</td>
              <td class="dim">{hardware(p)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="dbar">
        <button class="sc-primary" disabled={busy || !!joinWhy} onclick={submitJoin}>Show the plan</button>
        <button onclick={() => (join.open = false)}>Cancel</button>
        {#if joinWhy}<span class="dim">{joinWhy}</span>{/if}
      </div>
    </section>
  {/if}

  {#if promote.open}
    <section class="card">
      <h2>Promote workers</h2>
      <p class="dim">Workers become masters in pairs, so the control plane of {masters.length} stays odd.</p>
      <table class="pick">
        <tbody>
          {#each workers as w (w.id)}
            <tr>
              <td><input type="checkbox" aria-label="Promote {w.label}" checked={promote.nodes.includes(w.label)} onchange={() => toggle(promote.nodes, w.label)} /></td>
              <td class="mono">{w.label}</td>
              <td class="dim">{hardware(w)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="dbar">
        <button class="sc-primary" disabled={busy || !!promoteWhy} onclick={submitPromote}>Show the plan</button>
        <button onclick={() => (promote.open = false)}>Cancel</button>
        {#if promoteWhy}<span class="dim">{promoteWhy}</span>{/if}
      </div>
    </section>
  {/if}

  {#if inCluster}
    <h3>Members</h3>
    <table class="rows">
      <thead><tr><th>Node</th><th>Role</th><th>State</th><th>Address</th><th>Hardware</th><th></th></tr></thead>
      <tbody>
        {#each members as m (m.id)}
          {@const o = memberObj(m.label)}
          <tr class="health-{m.health}">
            <td>
              <span class="dot {m.health}" aria-hidden="true"></span>
              <span class="mono strong">{m.label}</span>
              {#if metric(m, 'CA')}<span class="tagchip">{metric(m, 'CA')}</span>{/if}
              {#if serving(m)}<span class="tagchip">storage</span>{/if}
            </td>
            <td>{metric(m, 'role')}</td>
            <td>
              {m.detail}{#if m.health === 'error'} <span class="bad">· not heard from</span>{/if}
              {@render objstatus(o)}
            </td>
            <td class="mono">{metric(m, 'address')}{#if metric(m, 'address')}<CopyButton value={metric(m, 'address')} label="Copy address" />{/if}</td>
            <td class="dim">{hardware(m)}</td>
            <td class="acts">
              {#if me.write && o && !o.deleting}
                {#if metric(m, 'role') === 'master'}<button disabled={busy} onclick={() => demote(m)}>Demote</button>{/if}
                {#if drained(m)}<button disabled={busy} onclick={() => drain(m, false)}>Uncordon</button>
                {:else}<button disabled={busy} onclick={() => drain(m, true)}>Drain</button>{/if}
                <button disabled={busy} onclick={() => storage(m, !serving(m))}>{serving(m) ? 'Stop storage' : 'Serve storage'}</button>
                <button class="danger" disabled={busy} onclick={() => release(m.label)}>Release</button>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  {#if requested.length}
    <h3>Asked for, not members yet</h3>
    <table class="rows">
      <thead><tr><th>Node</th><th>Asked</th><th>What stormcluster made of it</th><th></th></tr></thead>
      <tbody>
        {#each requested as o (o.name)}
          <tr>
            <td class="mono strong">{o.name}</td>
            <td>{o.spec?.role}{o.spec?.storage ? ', storage' : ''}</td>
            <td>{@render objstatus(o)}</td>
            <td class="acts">
              {#if me.write && !o.deleting}<button class="danger" disabled={busy} onclick={() => release(o.name)}>Withdraw</button>{/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  {#if cards}
    <h3>{inCluster ? 'Other nodes discovered' : 'Nodes discovered'}</h3>
    {#if peers.length}
      <table class="rows">
        <thead><tr><th>Node</th><th>What it is</th><th>Address</th><th>Release</th><th>Hardware</th></tr></thead>
        <tbody>
          {#each peers as p (p.id)}
            <tr class="health-{p.health}">
              <td><span class="dot {p.health}" aria-hidden="true"></span> <span class="mono strong">{p.label}</span>{#if p.label === self} <span class="tagchip">this node</span>{/if}</td>
              <td>{p.detail}{#if p.health === 'error'} <span class="bad">· stale</span>{/if}{#if metric(p, 'suggested name')} <span class="dim">· would be {metric(p, 'suggested name')}</span>{/if}</td>
              <td class="mono">{metric(p, 'address')}</td>
              <td class="mono">{metric(p, 'release')}{metric(p, 'edition') ? ` · ${metric(p, 'edition')}` : ''}</td>
              <td class="dim">{hardware(p)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <EmptyState icon="node" title="No other nodes heard" hint="stormcluster hears nodes through their announcements on the stormcast group; none has announced itself on this segment yet." />
    {/if}
  {/if}

  {#if ops.length}
    <h3>Operations</h3>
    <table class="rows">
      <thead><tr><th>Operation</th><th>Steps</th><th>Now</th><th></th></tr></thead>
      <tbody>
        {#each ops as o (o.id)}
          {@const id = opId(o)}
          <tr class="health-{o.health}">
            <td><span class="dot {o.health}" aria-hidden="true"></span> <span class="strong">{o.label}</span> <div class="dim mono">{id}</div></td>
            <td class="mono">{metric(o, 'steps')}{#if metric(o, 'warnings')} <span class="warn">· {metric(o, 'warnings')} warning(s)</span>{/if}</td>
            <td class:bad={o.health === 'error'}>{o.detail}</td>
            <td class="acts"><button onclick={() => toggleOp(o)}>{opened[id] ? 'Hide steps' : 'Steps'}</button></td>
          </tr>
          {#if opened[id]}
            <tr class="opsteps">
              <td colspan="4">
                {#if opened[id].error && !opened[id].steps}<span class="bad">{opened[id].error}</span>{/if}
                <ol class="steps">
                  {#each opened[id].steps || [] as s}
                    <li class="st-{s.status}">
                      <span class="status">{s.status}</span> {s.description}
                      {#if s.note}<span class="dim"> — {s.note}</span>{/if}
                      {#if s.error}<div class="bad">{s.error}</div>{/if}
                      {#if s.finishedAt}<span class="dim"> · {when(s.finishedAt)}</span>{/if}
                    </li>
                  {/each}
                </ol>
                {#if opened[id].warnings?.length}
                  <ul class="warnings">{#each opened[id].warnings as w}<li>{w}</li>{/each}</ul>
                {/if}
              </td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .error, .bad { color: var(--error); }
  .warn { color: var(--warn-strong); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .strong { font-weight: 600; }
  h3 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 18px 0 8px; }
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0 0 10px; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px var(--sc-row-px); margin-bottom: 12px; }
  .summary { display: flex; flex-direction: column; gap: 8px; }
  .summary.health-ok { box-shadow: inset 3px 0 var(--ok); }
  .summary.health-warn { box-shadow: inset 3px 0 var(--warn); }
  .summary.health-error { box-shadow: inset 3px 0 var(--error); }
  .metrics { display: flex; gap: 18px; flex-wrap: wrap; }
  .forms { display: flex; gap: 8px; }
  .dot { display: inline-block; width: 8px; height: 8px; border-radius: 50%; background: var(--text-faint); margin-right: 4px; }
  .dot.ok { background: var(--ok); }
  .dot.warn { background: var(--warn); }
  .dot.error { background: var(--error); }
  .dot.idle { background: var(--accent); }
  table.rows, table.pick { width: 100%; border-collapse: collapse; background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); margin-bottom: 12px; }
  table.pick { width: auto; min-width: 360px; }
  th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); padding: 8px 12px; border-bottom: 1px solid var(--border); }
  td { padding: 8px 12px; font-size: var(--sc-t-body); vertical-align: top; }
  tr + tr > td { border-top: 1px solid var(--sc-hairline); }
  tr.health-error td:first-child { box-shadow: inset 3px 0 var(--error); }
  .acts { white-space: nowrap; text-align: right; }
  .acts button { font-size: var(--sc-t-meta); margin: 2px 0 0 4px; }
  button.danger:not(:disabled) { color: var(--error); border-color: var(--error); }
  button.link { background: none; border: 0; padding: 0; color: var(--accent); font-size: var(--sc-t-meta); cursor: pointer; margin-left: auto; }
  .tagchip { font-size: var(--sc-t-eyebrow); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 0 5px; margin-left: 6px; color: var(--text-dim); }
  .outcome { display: flex; flex-wrap: wrap; gap: 8px; align-items: baseline; background: var(--panel); border: 1px solid var(--border); border-left: 3px solid var(--ok); border-radius: var(--radius); padding: 10px 12px; margin-bottom: 12px; }
  .outcome.bad { border-left-color: var(--error); }
  .reasons { flex-basis: 100%; margin: 4px 0 0; padding-left: 20px; }
  .reasons li { margin: 2px 0; }
  .scrim { position: fixed; inset: 0; background: rgb(0 0 0 / 0.4); z-index: 40; }
  .dialog { position: fixed; z-index: 41; top: 12vh; left: 50%; transform: translateX(-50%); width: min(560px, calc(100vw - 32px)); max-height: 76vh; overflow: auto; background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 16px 18px; box-shadow: 0 12px 40px rgb(0 0 0 / 0.35); }
  .steps { margin: 8px 0; padding-left: 22px; font-size: var(--sc-t-body); }
  .steps li { margin: 3px 0; }
  .steps .status { font-family: var(--mono); font-size: var(--sc-t-eyebrow); text-transform: uppercase; color: var(--text-faint); display: inline-block; min-width: 64px; }
  .st-done .status { color: var(--ok); }
  .st-running .status { color: var(--warn-strong); }
  .st-failed .status { color: var(--error); }
  .warnings { color: var(--warn-strong); font-size: var(--sc-t-body); padding-left: 20px; }
  .keep { display: flex; gap: 16px; margin: 6px 0 10px; font-size: var(--sc-t-body); }
  .keep input { width: auto; }
  .field { display: flex; gap: 8px; align-items: center; margin-bottom: 10px; font-size: var(--sc-t-body); }
  .field input { width: 220px; }
  .pick input[type='checkbox'] { width: auto; }
  .dbar { display: flex; gap: 8px; align-items: center; margin-top: 12px; }
  tr.opsteps td { background: var(--bg); }
  .objst { display: flex; flex-wrap: wrap; gap: 6px; align-items: baseline; margin-top: 4px; font-size: var(--sc-t-meta); }
  .phase { font-family: var(--mono); font-size: var(--sc-t-eyebrow); text-transform: uppercase; border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 0 5px; color: var(--text-dim); }
  .ph-ready { color: var(--ok); border-color: var(--ok); }
  .ph-blocked, .ph-failed { color: var(--error); border-color: var(--error); }
  .ph-joining, .ph-forming, .ph-promoting, .ph-demoting, .ph-draining, .ph-uncordoning, .ph-leaving, .ph-updating, .ph-dissolving, .ph-releasing { color: var(--warn-strong); border-color: var(--warn); }
  .op { flex-basis: 100%; }
  .blockers { color: var(--error); }
</style>
