<script>
  // Cluster (#63): what the cluster is made of, from stormcluster's feed —
  // the cluster (or this node as a single-node cluster), its members and
  // their roles, the nodes discovered that are not members, and the last
  // operations with their steps.
  //
  // Every change is an administrator's, and the plugin enforces it; the page
  // only decides whether to offer the buttons. Before anything runs the page
  // asks stormcluster for the plan (`?dryRun=true`) and shows its steps and
  // warnings — Split and Demote are not undone by pressing the button again.
  // A refusal is shown as stormcluster's reasons, every one of them.
  //
  // Two operations need input a body-less button cannot carry: forming a
  // cluster (a name, its masters, its workers) and joining nodes as masters
  // or promoting workers, which go in pairs so the control plane stays odd.
  // Those are forms that build the body of `POST /api/v1/operations`.
  import { onDestroy } from 'svelte'
  import { get } from '../api.js'
  import { noteActivity } from '../stores.svelte.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import CopyButton from '../components/CopyButton.svelte'

  const PROXY = '/api/plugins/cluster/proxy'
  const API = `${PROXY}/api/v1`

  let cards = $state(null)
  let me = $state({ admin: false, why: '' })
  let error = $state('')
  let timer = null

  // The answer of the last thing done: a message, or stormcluster's reasons.
  let outcome = $state(null)
  let busy = $state(false)
  // An open plan, waiting for Run or Cancel.
  let pending = $state(null)
  // Steps of each operation that has been opened, by id.
  let opened = $state({})

  async function load() {
    try {
      cards = await get(`${API}/components`)
      error = ''
      for (const id of Object.keys(opened)) readOp(id)
      const running = cards.find(isRunning)
      if (running && !opened[opId(running)]) readOp(opId(running))
    } catch (e) {
      error = `stormcluster did not answer: ${e.message}. It runs on every node at :9102; is it running on this one, or is [stormcluster] url pointing somewhere else?`
    }
    clearTimeout(timer)
    timer = setTimeout(load, 3000)
  }
  load()
  get('/api/plugins/cluster/me').then((d) => (me = d)).catch(() => {})
  onDestroy(() => clearTimeout(timer))

  const metric = (c, label) => c?.metrics?.find((m) => m.label === label)?.value ?? ''
  const opId = (c) => c.id.replace(/^op:/, '')
  // Running is the one operation with steps left that has not failed; a
  // finished one with warnings is warn too, but all its steps are done.
  function isRunning(c) {
    if (c.kind !== 'operation' || c.health !== 'warn') return false
    const [done, total] = String(metric(c, 'steps')).split('/').map(Number)
    return done < total
  }

  const system = $derived(cards?.find((c) => c.id === 'system'))
  const members = $derived((cards || []).filter((c) => c.kind === 'member'))
  const peers = $derived((cards || []).filter((c) => c.kind === 'peer'))
  const ops = $derived((cards || []).filter((c) => c.kind === 'operation'))
  const inCluster = $derived(members.length > 0)
  const masters = $derived(members.filter((m) => metric(m, 'role') === 'master'))
  const workers = $derived(members.filter((m) => metric(m, 'role') === 'worker'))
  // Peers that a form or a join can take: those whose own button is live.
  const available = $derived(peers.filter((p) => p.actions?.some((a) => a.enabled)))
  const runningOp = $derived(ops.some(isRunning))

  // --- talking to stormcluster ----------------------------------------
  async function send(path, body) {
    const opts = { method: 'POST' }
    if (body) {
      opts.headers = { 'Content-Type': 'application/json' }
      opts.body = JSON.stringify(body)
    }
    const r = await fetch(`${PROXY}${path}`, opts)
    const data = await r.json().catch(() => ({}))
    return { ok: r.ok, status: r.status, data }
  }
  const withQuery = (path, q) => `${path}${path.includes('?') ? '&' : '?'}${q}`

  function refused(title, data, status) {
    pending = null
    const reasons = data.refused?.length ? data.refused : [data.error || `stormcluster answered ${status}`]
    outcome = { title, reasons, coordinator: data.coordinator, bad: true }
    noteActivity({ reason: title, message: reasons.join('; '), source: 'Cluster', warning: true })
  }

  /// Ask for the plan of `path` (and `body`), and open it for confirmation.
  async function preview(title, path, body, extra = null) {
    busy = true
    outcome = null
    try {
      const { ok, status, data } = await send(withQuery(path, 'dryRun=true'), body)
      if (!ok) return refused(title, data, status)
      pending = { title, path, body, plan: data.plan || { steps: [], warnings: [] }, coordinator: data.coordinator, extra }
    } catch (e) {
      refused(title, { error: e.message }, 0)
    } finally {
      busy = false
    }
  }

  async function run(title, path, body) {
    busy = true
    outcome = null
    pending = null
    try {
      const { ok, status, data } = await send(path, body)
      if (!ok) return refused(title, data, status)
      const msg = data.id ? `started ${data.id}${data.coordinator ? ` on ${data.coordinator}` : ''}` : 'done'
      outcome = { title, message: msg }
      // The form that asked for it has done its job.
      form.open = join.open = promote.open = false
      noteActivity({ reason: title, message: msg, source: 'Cluster' })
      if (data.id) opened[data.id] = data
      await load()
    } catch (e) {
      refused(title, { error: e.message }, 0)
    } finally {
      busy = false
    }
  }

  function act(card, a) {
    const title = `${a.label}: ${card.label}`
    // Resume continues an operation whose plan is already on the page.
    if (a.id === 'resume') return run(title, a.path)
    preview(title, a.path, null, a.id === 'split' ? { keepData: true, base: a.path } : null)
  }

  // Split keeps the node's data unless told otherwise; the plan differs, so
  // choosing asks for it again.
  const splitPath = (base, keep) => (keep ? base : withQuery(base, 'keepData=false'))
  function setKeepData(keep) {
    const { title, extra } = pending
    preview(title, splitPath(extra.base, keep), null, { keepData: keep, base: extra.base })
  }
  function confirmPending() {
    const { title, path, body, extra } = pending
    run(title, extra?.base ? splitPath(extra.base, extra.keepData) : path, body)
  }

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
  // masters[0] is the seed: its CA and fastetcd become the cluster's.
  let form = $state({ open: false, name: '', masters: [], workers: [] })
  function openForm() {
    const self = (system?.label || '').replace(/ \(SNO\)$/, '')
    form = { open: true, name: 'storm', masters: self ? [self] : [], workers: [] }
  }
  function toggle(list, node) {
    const i = list.indexOf(node)
    if (i >= 0) list.splice(i, 1)
    else list.push(node)
  }
  function pickMaster(node) {
    toggle(form.masters, node)
    form.workers = form.workers.filter((w) => w !== node)
  }
  function pickWorker(node) {
    toggle(form.workers, node)
    form.masters = form.masters.filter((m) => m !== node)
  }
  const formWhy = $derived(
    !form.name.trim() ? 'name the cluster'
    : ![1, 3, 5].includes(form.masters.length) ? `${form.masters.length} master(s): the control plane must be 1, 3 or 5`
    : ''
  )
  function submitForm() {
    preview(`Form ${form.name}`, '/api/v1/operations', {
      op: 'form', name: form.name.trim(), masters: [...form.masters], workers: [...form.workers],
    })
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
    preview(`Join ${join.nodes.join(', ')} as ${join.role}s`, '/api/v1/operations', {
      op: 'join', nodes: [...join.nodes], role: join.role,
    })
  }
  function submitPromote() {
    preview(`Promote ${promote.nodes.join(', ')}`, '/api/v1/operations', { op: 'promote', nodes: [...promote.nodes] })
  }

  const peerName = (p) => p.label
  const hardware = (c) => ['cores', 'memory', 'drives'].map((k) => metric(c, k) && `${metric(c, k)} ${k === 'memory' ? '' : k}`.trim()).filter(Boolean).join(' · ')
  const when = (t) => (t ? new Date(t).toLocaleString() : '')
</script>

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Cluster' }, { label: 'Membership' }]} title={system?.label || 'Cluster'} count={cards ? members.length || null : null} />

  {#if !me.admin && cards}<p class="dim">{me.why}</p>{/if}
  {#if error}<p class="error">{error}</p>{/if}

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
      <p class="dim">
        Nothing has run yet. This is the plan stormcluster made{pending.coordinator ? ` on ${pending.coordinator}, which coordinates it` : ''}:
      </p>
      {#if pending.extra && 'keepData' in pending.extra}
        <div class="keep">
          <label><input type="radio" name="keep" checked={pending.extra.keepData} disabled={busy} onchange={() => setKeepData(true)} /> keep its data</label>
          <label><input type="radio" name="keep" checked={!pending.extra.keepData} disabled={busy} onchange={() => setKeepData(false)} /> wipe its data</label>
        </div>
      {/if}
      <ol class="steps">
        {#each pending.plan.steps as s}<li>{s.description}</li>{/each}
      </ol>
      {#if pending.plan.warnings?.length}
        <ul class="warnings">{#each pending.plan.warnings as w}<li>{w}</li>{/each}</ul>
      {/if}
      <div class="dbar">
        <button class="sc-primary" disabled={busy} onclick={confirmPending}>Run {pending.plan.steps.length} step{pending.plan.steps.length === 1 ? '' : 's'}</button>
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
      {#if me.admin}
        <div class="forms">
          {#if !inCluster}
            <button disabled={busy || runningOp || !available.length} onclick={openForm}>Form a cluster…</button>
          {:else}
            <button disabled={busy || runningOp || !available.length} onclick={() => (join = { open: true, role: 'worker', nodes: [] })}>Join nodes…</button>
            <button disabled={busy || runningOp || !workers.length} onclick={() => (promote = { open: true, nodes: [] })}>Promote workers…</button>
          {/if}
        </div>
      {/if}
    </section>
  {/if}

  {#if form.open}
    <section class="card">
      <h2>Form a cluster</h2>
      <p class="dim">The first master is the seed: its CA, fastetcd and data become the cluster's. The control plane is 1, 3 or 5 masters.</p>
      <label class="field">Name <input bind:value={form.name} aria-label="Cluster name" /></label>
      <table class="pick">
        <thead><tr><th>Node</th><th>Master</th><th>Worker</th><th></th></tr></thead>
        <tbody>
          {#each available as p (p.id)}
            <tr>
              <td class="mono">{peerName(p)}{#if form.masters[0] === peerName(p)} <span class="tagchip">seed</span>{/if}</td>
              <td><input type="checkbox" aria-label="{peerName(p)} as master" checked={form.masters.includes(peerName(p))} onchange={() => pickMaster(peerName(p))} /></td>
              <td><input type="checkbox" aria-label="{peerName(p)} as worker" checked={form.workers.includes(peerName(p))} onchange={() => pickWorker(peerName(p))} /></td>
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
              <td><input type="checkbox" aria-label="Join {peerName(p)}" checked={join.nodes.includes(peerName(p))} onchange={() => toggle(join.nodes, peerName(p))} /></td>
              <td class="mono">{peerName(p)}</td>
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

  {#snippet actions(c)}
    {#if me.admin}
      {#each c.actions || [] as a (a.id)}
        <button class:danger={a.danger} disabled={busy || !a.enabled} onclick={() => act(c, a)}>{a.label}</button>
      {/each}
    {/if}
  {/snippet}

  {#if inCluster}
    <h3>Members</h3>
    <table class="rows">
      <thead><tr><th>Node</th><th>Role</th><th>State</th><th>Address</th><th>Hardware</th><th></th></tr></thead>
      <tbody>
        {#each members as m (m.id)}
          <tr class="health-{m.health}">
            <td>
              <span class="dot {m.health}" aria-hidden="true"></span>
              <span class="mono strong">{m.label}</span>
              {#if metric(m, 'CA')}<span class="tagchip">{metric(m, 'CA')}</span>{/if}
            </td>
            <td>{metric(m, 'role')}</td>
            <td>{m.detail}{#if m.health === 'error'} <span class="bad">· not heard from</span>{/if}</td>
            <td class="mono">{metric(m, 'address')}{#if metric(m, 'address')}<CopyButton value={metric(m, 'address')} label="Copy address" />{/if}</td>
            <td class="dim">{hardware(m)}</td>
            <td class="acts">{@render actions(m)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  {#if cards}
    <h3>{inCluster ? 'Other nodes discovered' : 'Nodes discovered'}</h3>
    {#if peers.length}
      <table class="rows">
        <thead><tr><th>Node</th><th>What it is</th><th>Address</th><th>Release</th><th>Hardware</th><th></th></tr></thead>
        <tbody>
          {#each peers as p (p.id)}
            <tr class="health-{p.health}">
              <td><span class="dot {p.health}" aria-hidden="true"></span> <span class="mono strong">{p.label}</span></td>
              <td>{p.detail}{#if p.health === 'error'} <span class="bad">· stale</span>{/if}</td>
              <td class="mono">{metric(p, 'address')}</td>
              <td class="mono">{metric(p, 'release')}{metric(p, 'edition') ? ` · ${metric(p, 'edition')}` : ''}</td>
              <td class="dim">{hardware(p)}</td>
              <td class="acts">{@render actions(p)}</td>
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
            <td class="acts">
              <button onclick={() => toggleOp(o)}>{opened[id] ? 'Hide steps' : 'Steps'}</button>
              {@render actions(o)}
            </td>
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
</style>
