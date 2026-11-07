<script>
  // Formatting drives, and following a format (#165).
  //
  // A format on a 10 TB SAS drive takes most of a day, so this is mostly a
  // page to come back to: the runs are stormdrive's persisted records, read
  // again every few seconds, and nothing about them lives in this tab or in
  // the console. Starting one is destructive storage (#82): the apiserver
  // decides who may, the confirmation names each drive, and the request goes
  // as the person asking.
  import { get } from '../api.js'
  import { route } from '../router.svelte.js'
  import { formatBytes } from '../drivemap.js'
  import { badgeOf, byNode, currentText, defaultOption, duration, finished, formatText, isRunning, planFor, readBack, resultText, selectable, timing } from '../driveformat.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import FormatDialog from '../components/FormatDialog.svelte'
  import FormatProgress from '../components/FormatProgress.svelte'

  let data = $state(null)
  let error = $state('')
  let guard = $state(null)
  let now = $state(Math.floor(Date.now() / 1000))
  let picked = $state({})
  // The Format dialog (#308), open on the drives ticked here.
  let dialog = $state(null)
  let outcome = $state(null)
  let history = $state(null)

  async function refresh() {
    try {
      data = await get('/api/plugins/drive/formats')
      error = ''
    } catch (e) {
      error = e.message
    }
  }
  async function ask() {
    try {
      const g = await get(`/api/v1/console/guard?method=POST&path=${encodeURIComponent('/api/plugins/drive/format')}`)
      guard = g.guard
    } catch (e) {
      guard = { allowed: false, reason: e.message }
    }
  }
  $effect(() => {
    refresh()
    ask()
    const poll = setInterval(refresh, 5000)
    const tick = setInterval(() => (now = Math.floor(Date.now() / 1000)), 1000)
    return () => {
      clearInterval(poll)
      clearInterval(tick)
    }
  })

  const drives = $derived(data?.drives || [])
  const nodes = $derived(byNode(drives))
  const running = $derived(drives.filter(isRunning))
  const ended = $derived(finished(drives))
  const silent = $derived((data?.nodes || []).filter((n) => !n.ok))
  const noPlans = $derived((data?.nodes || []).filter((n) => n.ok && !n.plans))
  const allowed = $derived(!!guard?.allowed)
  const key = (r) => `${r.node}/${r.id}`
  const selection = $derived(drives.filter((r) => picked[key(r)] && selectable(r)))
  // Picked on the Drives page (#308): `?pick=node/id,…` opens the dialog on them.
  let fromRoute = false
  $effect(() => {
    if (fromRoute || !data) return
    fromRoute = true
    const want = (route.current.query.get('pick') || '').split(',').filter(Boolean)
    if (want.length) dialog = { picked: want }
  })

  function plan(r) {
    return planFor(r, defaultOption(r))
  }
  function fallbackText(c) {
    return c.fallbacks.map((f) => formatText(f.blockSize, f.protection)).join(', then ')
  }
  async function done(results) {
    outcome = results
    dialog = null
    picked = {}
    await refresh()
  }

  async function showHistory(r) {
    history = { drive: r, runs: null, error: '' }
    try {
      const h = await get(`/api/plugins/drive/formats/history?node=${encodeURIComponent(r.node)}&id=${encodeURIComponent(r.id)}`)
      history = { drive: r, runs: h.runs, active: h.active, error: '' }
    } catch (e) {
      history = { drive: r, runs: [], error: e.message }
    }
  }
  const paramText = (p) => formatText(p?.block_size, p?.protection)
  const afterText = (a) => (a?.block_size ? `${a.block_size}${a.prot_type ? ` + PI type ${a.prot_type}` : ', no PI'}` : '—')
</script>

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Hardware' }, { label: 'Drives', href: '#/drives' }, { label: 'Format' }]} title="Format drives" count={data ? drives.length : null} />

  {#if error}<p class="error">{error}</p>{/if}
  {#each silent as n (n.node)}<p class="warn">{n.node}: {n.error}</p>{/each}
  {#each noPlans as n (n.node)}<p class="warn">{n.node}: this stormdrive is older than v0.39.0, so it has no plan and no record of a run to show.</p>{/each}
  {#if guard && !allowed}
    <p class="note" data-testid="refused">You can follow formats here, and not start one. {guard.reason}.</p>
  {/if}

  {#if running.length}
    <section class="card" data-testid="running">
      <h2>Formatting now</h2>
      <FormatProgress rows={running} {now} />
    </section>
  {/if}

  {#if outcome}
    <section class="card" data-testid="outcome">
      <h2>What was asked</h2>
      {#each outcome as o}
        <p class:error={!o.ok}>{o.node}, {o.drives.length} {o.drives.length === 1 ? 'drive' : 'drives'}, {formatText(o.block_size, o.protection)}: {o.ok ? 'started' : `refused, nothing on this request started. ${o.error}`}</p>
      {/each}
    </section>
  {/if}

  {#if data && !drives.length}
    <EmptyState icon="storage" title="No drives" hint="No stormdrive in this cluster reports a drive." />
  {:else if drives.length}
    <div class="choose">
      <button class="danger" disabled={!allowed} onclick={() => (dialog = { picked: selection.map(key) })} data-testid="open-dialog">
        {selection.length ? `Format ${selection.length} ${selection.length === 1 ? 'drive' : 'drives'}…` : 'Format drives…'}
      </button>
      <span class="dim">Tick drives below, or choose them in the dialog. A drive in use is never offered.</span>
    </div>

    {#if dialog}
      <FormatDialog records={drives} picked={dialog.picked} {guard} onclose={() => (dialog = null)} ondone={done} />
    {/if}

    {#each nodes as n (n.node)}
      <section class="card">
        <h2>{n.node}</h2>
        <table class="drives">
          <thead><tr>{#if allowed}<th></th>{/if}<th>Bay</th><th>Drive</th><th>Serial / WWN</th><th>Size</th><th>Use</th><th>Now</th><th>Would become</th><th>Can it be formatted</th><th></th></tr></thead>
          <tbody>
            {#each n.drives as r (key(r))}
              {@const c = plan(r)}
              <tr class:off={!selectable(r)}>
                {#if allowed}
                  <td><input type="checkbox" aria-label="Choose {r.name}" disabled={!selectable(r)} bind:checked={picked[key(r)]} /></td>
                {/if}
                <td>{r.bay ?? '—'}</td>
                <td>{r.name} <span class="dim">{r.model}</span></td>
                <td class="mono">{r.serial}{#if r.wwn}<div class="dim">{r.wwn}</div>{/if}</td>
                <td>{r.capacity ? formatBytes(r.capacity) : '—'}</td>
                <td data-testid="use">{#if badgeOf(r)}<span class="badge" class:system={r.use?.use === 'system'} title={r.use?.reason}>{badgeOf(r)}</span>{#each r.use?.depends_on || [] as dep}<div class="dim">{dep}</div>{/each}{:else}<span class="dim">free</span>{/if}</td>
                <td>{currentText(r)}</td>
                <td>
                  {resultText(c.block_size, c.protection)}
                  {#if c.basis === 'try'}<span class="dim" title="The drive does not list its formats, so the run finds out">(a try)</span>{/if}
                  {#if c.fallbacks.length}<div class="dim">if refused: {fallbackText(c)}</div>{/if}
                  {#if r.planned?.error}<div class="dim">{r.planned.error}</div>{/if}
                </td>
                <td class="why">
                  {#if isRunning(r)}formatting{:else if r.whyNot}No: {r.whyNot}{:else}Yes{/if}
                  {#if r.slabs}<div class="dim">holds {r.slabs} {r.slabs === 1 ? 'slab' : 'slabs'}</div>{/if}
                </td>
                <td><button class="link" onclick={() => showHistory(r)}>History</button></td>
              </tr>
            {/each}
          </tbody>
        </table>
      </section>
    {/each}

    {#if ended.length}
      <section class="card" data-testid="ended">
        <h2>Last run of each drive</h2>
        <table>
          <thead><tr><th>Drive</th><th>Result</th><th>Asked</th><th>Reads back</th><th>Took</th><th>Finished</th></tr></thead>
          <tbody>
            {#each ended as r (key(r))}
              {@const f = r.format}
              {@const back = readBack(f)}
              <tr>
                <td>{r.node} · {r.name} <span class="mono dim">{r.serial}</span></td>
                <td class:bad={f.state !== 'done'}>{f.state}{#if f.error}<div class="small">{f.error}</div>{/if}</td>
                <td>{formatText(f.toBlockSize, f.protection)}{#each f.refused || [] as why}<div class="dim">refused: {why}</div>{/each}</td>
                <td class:bad={back && !back.matches}>{back ? back.text : '—'}{#if back && !back.matches}<div class="small">not what was asked</div>{/if}</td>
                <td>{duration(timing(f, now).elapsed)}</td>
                <td>{f.finishedUnix ? new Date(f.finishedUnix * 1000).toLocaleString() : '—'}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </section>
    {/if}

    {#if history}
      <section class="card" data-testid="history">
        <h2>History of {history.drive.node} · {history.drive.name}</h2>
        {#if history.error}<p class="error">{history.error}</p>
        {:else if history.runs === null}<p class="dim">Loading…</p>
        {:else if !history.runs.length}<p class="dim">{history.active === false ? 'This node keeps no drive history: system-data is not there.' : 'No format is recorded for this drive.'}</p>
        {:else}
          <table>
            <thead><tr><th>When</th><th>Result</th><th>Asked</th><th>Reads back</th><th>By</th></tr></thead>
            <tbody>
              {#each history.runs as h}
                <tr>
                  <td>{h.at}</td>
                  <td class:bad={h.result !== 'done'}>{h.result}{#if h.error}<div class="small">{h.error}</div>{/if}</td>
                  <td>{paramText(h.params)}</td>
                  <td>{afterText(h.after)}</td>
                  <td>{h.requester || '—'}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </section>
    {/if}
  {:else if !error}
    <div class="sc-empty"><p>Asking every stormdrive…</p></div>
  {/if}
</div>

<style>
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px var(--sc-row-px); margin-bottom: 12px; overflow-x: auto; }
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0 0 10px; }
  p { font-size: var(--sc-t-body); margin: 0 0 8px; }
  .error { color: var(--error); }
  .warn { color: var(--warn-strong); font-size: var(--sc-t-body); margin: 0 0 8px; }
  .small { font-size: var(--sc-t-meta); margin: 2px 0 0; }
  .note { padding: 7px 12px; font-size: var(--sc-t-meta); color: var(--text-dim); background: var(--panel); border: 1px solid var(--border); border-left: 3px solid var(--accent); border-radius: var(--radius-sm); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .choose { display: flex; gap: 14px; align-items: center; flex-wrap: wrap; margin-bottom: 12px; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .badge { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; padding: 1px 6px; border-radius: 999px; border: 1px solid var(--warn-strong); color: var(--warn-strong); white-space: nowrap; }
  .badge.system { border-color: var(--error); color: var(--error); }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); padding: 4px 10px 4px 0; white-space: nowrap; }
  td { padding: 6px 10px 6px 0; border-top: 1px solid var(--sc-hairline); vertical-align: top; }
  tr.off td { color: var(--text-dim); }
  .why { max-width: 280px; }
  .bad { color: var(--error); }
  button.danger { color: var(--error); }
  button.link { background: none; border: none; padding: 0; color: var(--accent); }
</style>
