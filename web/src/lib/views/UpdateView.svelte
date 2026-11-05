<script>
  // The Update page (#164). The owner, on forge1's first update: "update needs a ui as
  // well". It ran with nothing to see while stormupdate sat in a pre-check, so this
  // puts first what it is doing now, and how long it has been there.
  //
  // It is the cluster's one ClusterUpdate, as any node's stormupdate answers it. An
  // admin's acts go through the console, which holds the update token and records them.
  import { onDestroy } from 'svelte'
  import { get, postJson } from '../api.js'
  import { ago } from '../ui/time.js'
  import { nodeSteps, nodeLine, duration, actions, releases, historyRows, imageRows, changeLine, running } from '../update.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import StatusPill from '../components/StatusPill.svelte'

  const API = '/api/plugins/update'
  let data = $state(null)
  let error = $state('')
  let acting = $state('')
  let actError = $state('')
  let now = $state(Date.now() / 1000)
  let timer = null

  async function load() {
    try {
      data = await get(`${API}/snapshot`)
      error = ''
    } catch (e) {
      error = `the console did not answer: ${e.message}`
    }
    clearTimeout(timer)
    timer = setTimeout(load, running(st?.phase) ? 2000 : 8000)
  }
  load()
  const tick = setInterval(() => (now = Date.now() / 1000), 1000)
  onDestroy(() => {
    clearTimeout(timer)
    clearInterval(tick)
  })

  const spec = $derived(data?.update?.spec)
  const st = $derived(data?.update?.status)
  const rel = $derived(releases(st, data?.generations))
  const may = $derived(actions(spec, st))
  const canAct = $derived(data?.admin && data?.can_write)
  const when = (unix) => (unix ? `${ago(new Date(unix * 1000).toISOString())} ago` : '—')
  const stamp = (unix) => (unix ? new Date(unix * 1000).toISOString() : '')
  const phaseHealth = (p) => ({ Failed: 'error', RolledBack: 'warn', RollingBack: 'warn', Paused: 'warn', Completed: 'ok', Progressing: 'ok' })[p] || 'idle'

  // Every act asks first, in a sentence that says what will happen.
  async function act(what, path, body, question) {
    if (!window.confirm(question)) return
    acting = what
    actError = ''
    try {
      await postJson(`${API}${path}`, body || {})
      await load()
    } catch (e) {
      actError = e.message
      await load()
    } finally {
      acting = ''
    }
  }
  const start = (v) => act(`start ${v}`, '/upgrade', { release: v }, `Update this cluster to ${v}?\n\nEvery node moves to it in turn, and each reboots. A node that fails its checks is rolled back and the run stops.`)
  const pause = () => act('pause', '/pause', null, 'Pause this update?\n\nIt stops at the next node. A node already moving finishes.')
  const resume = () => act('resume', '/resume', null, 'Resume this update?')
  const rollback = () => act('rollback', '/rollback', null, `Roll this cluster back to ${rel.previous}?\n\nEvery node on ${st?.targetRelease || 'the new release'} moves back to it and reboots.`)
</script>

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Compute' }, { label: 'Update' }]} title="Update" scope="this cluster">
    {#snippet status()}{#if st}<StatusPill health={phaseHealth(st.phase)} label={st.phase || 'Idle'} />{/if}{/snippet}
  </PageHeader>

  {#if error}<p class="bad">{error}</p>{/if}
  {#if data && !data.update}
    <EmptyState icon="overview" title={data.not_started ? 'stormupdate is not started on this node' : 'stormupdate did not answer'} hint={data.error} />
  {:else if data}
    <section class="band">
      <div><span class="k">Running</span> <strong class="mono" class:warn={rel.mixed}>{rel.running || '—'}</strong>{#if rel.mixed}<span class="small warn gap">nodes differ</span>{/if}</div>
      {#if rel.target}<div><span class="k">Moving to</span> <strong class="mono">{rel.target}</strong></div>{/if}
      <div><span class="k">Staged here</span> <strong class="mono">{rel.staged || '—'}</strong>{#if rel.staged && !rel.stagedComplete}<span class="small warn gap">not whole yet</span>{/if}</div>
      {#if rel.previous}<div><span class="k">Before it</span> <strong class="mono">{rel.previous}</strong></div>{/if}
      {#if rel.rolledBackFrom}<div><span class="k">Rolled back from</span> <strong class="mono" class:warn={rel.rolledBackFromNow}>{rel.rolledBackFrom}</strong></div>{/if}
      <div><span class="k">Flavor</span> {st?.flavor || '—'}</div>
    </section>

    {#if actError}<p class="bad" role="alert">{actError}</p>{/if}
    {#if !data.admin}
      <p class="dim small">Starting, pausing and rolling back are an administrator's.</p>
    {:else if !data.can_write}
      <p class="warn small">Update actions are not set up on this node: the console holds no update token (stormcos#789). What it is doing still shows here.</p>
    {/if}

    {#if st?.message || running(st?.phase)}
      <section class="card now" aria-label="Now">
        <div class="head"><h2>Now</h2>
          {#if data.message_since}<span class="dim small" title="since this console first saw the message; stormupdate keeps no clock on it (stormupdate#37)">this message for {duration(Math.round(now - data.message_since))}</span>{/if}
        </div>
        <p class:bad={st?.phase === 'Failed'} class="msg">{st?.message || st?.phase}</p>
        {#if canAct && (may.pause || may.resume || (may.rollback && running(st?.phase)))}
          <div class="row">
            {#if may.pause}<button disabled={!!acting} onclick={pause}>Pause at the next node</button>{/if}
            {#if may.resume}<button disabled={!!acting} onclick={resume}>Resume</button>{/if}
            {#if may.rollback && running(st?.phase)}<button class="danger" disabled={!!acting} onclick={rollback}>Roll back to {rel.previous}</button>{/if}
          </div>
        {/if}
      </section>
    {/if}

    {#if st?.nodes?.length}
      <section class="card" aria-label="Nodes">
        <h2>Nodes</h2>
        {#each st.nodes as n (n.name)}
          <div class="node">
            <div class="head">
              <strong>{n.name}</strong>
              {#if n.controlPlane}<span class="tag">control plane</span>{/if}
              <span class="mono dim">{n.release || '—'}</span>
              {#if n.since && !['Done', 'Pending'].includes(n.phase)}<span class="dim small" title={stamp(n.since)}>here for {duration(Math.round(now - n.since))}</span>{/if}
              {#if n.updateSeconds}<span class="dim small">took {duration(n.updateSeconds)}</span>{/if}
            </div>
            <ol class="steps">
              {#each nodeSteps(n) as s (s.step)}<li class={s.state} aria-current={s.state === 'now' ? 'step' : undefined}>{s.word}</li>{/each}
            </ol>
            {#if nodeLine(n)}<p class="small line" class:bad={n.phase === 'Failed'}>{nodeLine(n)}</p>{/if}
          </div>
        {/each}
      </section>
    {/if}

    {#if st?.images?.length && running(st?.phase)}
      <section class="card">
        <h2>Images of {st.targetRelease}</h2>
        <table>
          <thead><tr><th>Image</th><th>Data</th><th>Nodes</th></tr></thead>
          <tbody>
            {#each imageRows(st) as i (i.name)}
              <tr>
                <td class="mono">{i.name}</td>
                <td class="small">{i.policy}</td>
                <td class="small">{#each i.states as s, k}{k ? ', ' : ''}{s.count} {s.state}{/each}{#each i.failed as f}<div class="bad">{f}</div>{/each}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </section>
    {/if}

    <section class="card" aria-label="Offered">
      <h2>Offered</h2>
      {#if !st?.available?.length}
        <p class="dim">Nothing newer is offered to this cluster.</p>
      {:else}
        {#each st.available as a (a.version)}
          <div class="offer">
            <div class="head">
              <strong class="mono">{a.version}</strong>
              {#if a.publishedAt}<span class="dim small" title={stamp(a.publishedAt)}>published {when(a.publishedAt)}</span>{/if}
              {#if canAct}<button class="primary" disabled={!may.start || !!acting} title={may.start ? '' : 'an update is in progress'} onclick={() => start(a.version)}>Update to {a.version}</button>{/if}
            </div>
            {#if a.notes}<p class="small notes">{a.notes}</p>{/if}
            {#if a.changed?.length}
              <details><summary class="small">{a.changed.length} changed</summary>
                <ul class="plain small mono">{#each a.changed as c}<li>{changeLine(c)}</li>{/each}</ul>
              </details>
            {/if}
          </div>
        {/each}
      {/if}
      {#if canAct && may.rollback && !running(st?.phase)}
        <div class="row back"><button class="danger" disabled={!!acting} onclick={rollback}>Roll back to {rel.previous}</button></div>
      {/if}
    </section>

    <section class="card" aria-label="History">
      <h2>History</h2>
      {#if !st?.history?.length}
        <p class="dim">No update has run on this cluster.</p>
      {:else}
        <table>
          <thead><tr><th>Started</th><th>From</th><th>To</th><th>Took</th><th>Result</th><th>Why</th></tr></thead>
          <tbody>
            {#each historyRows(st) as h (h.started)}
              <tr>
                <td class="num" title={stamp(h.started)}>{when(h.started)}</td>
                <td class="mono">{h.from}</td>
                <td class="mono">{h.to}</td>
                <td class="num">{h.took != null ? duration(h.took) : '—'}</td>
                <td><StatusPill health={h.tone} label={h.result} /></td>
                <td class="small">{h.message || ''}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
      <p class="dim small note">A run refused before any node moved is not kept here yet (stormupdate#37): while it is the latest, it shows under Now.</p>
    </section>

    {#if data.record?.length}
      <section class="card" aria-label="Asked here">
        <h2>Asked on this console</h2>
        <table>
          <thead><tr><th>When</th><th>Who</th><th>What</th><th>Answer</th></tr></thead>
          <tbody>
            {#each data.record as r}
              <tr>
                <td class="num" title={r.at}>{ago(r.at)} ago</td>
                <td>{r.user}</td>
                <td>{r.action}{r.release ? ` ${r.release}` : ''}</td>
                <td class="small" class:bad={!r.ok}>{r.ok ? 'done' : r.error || r.code}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </section>
    {/if}
  {/if}
</div>

<style>
  .small { font-size: var(--sc-t-meta); }
  .gap { margin-left: 8px; }
  .dim { color: var(--text-dim); }
  .warn { color: var(--warn-strong); }
  .bad { color: var(--error); }
  .mono { font-family: var(--mono); }
  .band { display: flex; gap: 28px; flex-wrap: wrap; background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 12px var(--sc-row-px); margin-bottom: 12px; }
  .band .k, h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); }
  .band .k { display: block; margin-bottom: 2px; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 12px var(--sc-row-px); margin-bottom: 12px; min-width: 0; }
  .card.now { border-color: var(--accent); }
  h2 { margin: 0 0 8px; }
  .head { display: flex; gap: 12px; align-items: baseline; flex-wrap: wrap; }
  .head h2 { margin: 0; }
  .msg { margin: 6px 0; overflow-wrap: anywhere; }
  .row { display: flex; gap: 8px; flex-wrap: wrap; }
  .back { margin-top: 10px; }
  button.danger { color: var(--error); }
  .tag { font-size: var(--sc-t-eyebrow); padding: 1px 6px; border-radius: 999px; border: 1px solid var(--border); color: var(--text-dim); }
  .node { padding: 8px 0; }
  .node + .node { border-top: 1px solid var(--sc-hairline); }
  .steps { display: flex; flex-wrap: wrap; gap: 4px 0; list-style: none; margin: 8px 0 4px; padding: 0; font-size: var(--sc-t-meta); }
  .steps li { padding: 2px 10px; color: var(--text-faint); border-left: 2px solid var(--border); }
  .steps li.done { color: var(--text-dim); border-left-color: var(--ok); }
  .steps li.now { color: var(--text); font-weight: 600; border-left-color: var(--accent); }
  .steps li.warn { color: var(--warn-strong); font-weight: 600; border-left-color: var(--warn-strong); }
  .steps li.bad { color: var(--error); font-weight: 600; border-left-color: var(--error); }
  .line { margin: 2px 0 0; overflow-wrap: anywhere; }
  .offer { padding: 6px 0; }
  .offer + .offer { border-top: 1px solid var(--sc-hairline); }
  .offer button { margin-left: auto; }
  .notes { margin: 4px 0; white-space: pre-wrap; }
  .plain { list-style: none; margin: 4px 0; padding: 0; }
  .note { margin: 8px 0 0; }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); padding: 5px 10px; border-bottom: 1px solid var(--border); }
  td { padding: 5px 10px; vertical-align: top; overflow-wrap: anywhere; }
  tr + tr > td { border-top: 1px solid var(--sc-hairline); }
  td.num { font-variant-numeric: tabular-nums; font-family: var(--mono); font-size: var(--sc-t-meta); white-space: nowrap; }
</style>
