<script>
  // Formatting drives, and following a format (#165).
  //
  // A format on a 10 TB SAS drive takes most of a day, so this is mostly a
  // page to come back to: the runs are stormdrive's persisted records, read
  // again every few seconds, and nothing about them lives in this tab or in
  // the console. Starting one is destructive storage (#82): the apiserver
  // decides who may, the confirmation names each drive, and the request goes
  // as the person asking.
  import { get, postJson } from '../api.js'
  import { noteActivity } from '../stores.svelte.js'
  import { formatBytes } from '../drivemap.js'
  import { byNode, chosen, currentText, duration, finished, formatText, isRunning, phrase, readBack, request, selectable, timing } from '../driveformat.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'

  let data = $state(null)
  let error = $state('')
  let guard = $state(null)
  let now = $state(Math.floor(Date.now() / 1000))
  let picked = $state({})
  let blockSize = $state(4096)
  let pi = $state('offered')
  let asking = $state(false)
  let typed = $state('')
  let busy = $state(false)
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
  const word = $derived(phrase(selection))

  function plan(r) {
    return chosen(r, blockSize, pi)
  }
  function fallbackText(c) {
    return c.fallbacks.map((f) => formatText(f.blockSize, f.protection)).join(', then ')
  }

  async function start() {
    busy = true
    error = ''
    outcome = null
    try {
      const body = request(selection, blockSize, pi, typed.trim())
      const r = await postJson('/api/plugins/drive/format', body, 'POST', { 'X-Storm-Confirm': guard?.confirm || 'format' })
      outcome = r.results || []
      for (const o of outcome) {
        const what = `${o.drives.length} ${o.drives.length === 1 ? 'drive' : 'drives'} on ${o.node}: ${formatText(o.block_size, o.protection)}`
        noteActivity({ reason: 'Format', message: o.ok ? `started, ${what}` : `refused, ${what}: ${o.error}`, source: `drives/${o.node}`, warning: !o.ok })
      }
      asking = false
      typed = ''
      picked = {}
      await refresh()
    } catch (e) {
      error = e.message
    }
    busy = false
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
      {#each running as r (key(r))}
        {@const f = r.format}
        {@const t = timing(f, now)}
        <div class="run">
          <div class="run-head">
            <strong>{r.node} · {r.name}</strong>
            <span class="dim">bay {r.bay ?? '—'} · {r.model} · {r.serial}</span>
            <span class="to">to {formatText(f.toBlockSize, f.protection)}</span>
          </div>
          <div class="bar" role="progressbar" aria-valuenow={f.progressPct ?? 0} aria-valuemin="0" aria-valuemax="100"><i style="width: {f.progressPct ?? 0}%"></i></div>
          <div class="facts">
            <span class="pct">{f.progressPct === null || f.progressPct === undefined ? 'no progress reported yet' : `${f.progressPct}%`}</span>
            <span>{f.phase}</span>
            <span>elapsed {duration(t.elapsed)}</span>
            <span>{t.left === null ? 'no estimate yet' : `about ${duration(t.left)} left`}</span>
          </div>
          {#each f.refused || [] as why}<p class="warn small">The drive refused a format: {why}</p>{/each}
          {#each r.resets || [] as line}<p class="warn small">Controller reset: {line}</p>{/each}
        </div>
      {/each}
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
    {#if allowed}
      <div class="choose">
        <label>Block size
          <select bind:value={blockSize} aria-label="Block size">
            <option value={4096}>4096</option>
            <option value={512}>512</option>
          </select>
        </label>
        <label>Protection
          <select bind:value={pi} aria-label="Protection">
            <option value="offered">PI type 1 where offered</option>
            <option value="none">None</option>
          </select>
        </label>
        <button class="danger" disabled={!selection.length || asking} onclick={() => { asking = true; typed = '' }}>
          Format {selection.length || ''} {selection.length === 1 ? 'drive' : 'drives'}
        </button>
      </div>
    {/if}

    {#if asking && selection.length}
      <section class="card confirm" role="dialog" aria-label="Confirm the format">
        <h2>Format {selection.length} {selection.length === 1 ? 'drive' : 'drives'}: their data goes</h2>
        <p>Everything on these drives is destroyed and cannot be recovered. A large drive takes most of a day, and a format cannot be cancelled. It is done as you.</p>
        <table>
          <thead><tr><th>Drive</th><th>Bay</th><th>Serial</th><th>Now</th><th>Becomes</th><th>If the drive refuses</th></tr></thead>
          <tbody>
            {#each selection as r (key(r))}
              {@const c = plan(r)}
              <tr>
                <td>{r.node} · {r.name} <span class="dim">{r.model}</span></td>
                <td>{r.bay ?? '—'}</td>
                <td class="mono">{r.serial}</td>
                <td>{currentText(r)}</td>
                <td>{formatText(c.block_size, c.protection)}</td>
                <td>{c.fallbacks.length ? fallbackText(c) : 'nothing: the run ends'}</td>
              </tr>
            {/each}
          </tbody>
        </table>
        <label class="type">Type <span class="mono">{word}</span> to confirm
          <input bind:value={typed} aria-label="Confirmation" autocomplete="off" />
        </label>
        <div class="row">
          <button class="danger" disabled={busy || typed.trim() !== word} onclick={start}>Format</button>
          <button disabled={busy} onclick={() => (asking = false)}>Cancel</button>
        </div>
      </section>
    {/if}

    {#each nodes as n (n.node)}
      <section class="card">
        <h2>{n.node}</h2>
        <table class="drives">
          <thead><tr>{#if allowed}<th></th>{/if}<th>Bay</th><th>Drive</th><th>Serial / WWN</th><th>Size</th><th>Now</th><th>Would become</th><th>Can it be formatted</th><th></th></tr></thead>
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
                <td>{currentText(r)}</td>
                <td>
                  {formatText(c.block_size, c.protection)}
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
  .run + .run { margin-top: 14px; }
  .run-head { display: flex; gap: 10px; align-items: baseline; flex-wrap: wrap; font-size: var(--sc-t-body); }
  .to { margin-left: auto; color: var(--text-dim); font-size: var(--sc-t-meta); }
  .bar { height: 8px; background: var(--panel-raised); border-radius: 999px; overflow: hidden; margin: 6px 0; }
  .bar i { display: block; height: 100%; background: var(--accent); }
  .facts { display: flex; gap: 16px; flex-wrap: wrap; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .pct { font-weight: 600; color: var(--text); font-variant-numeric: tabular-nums; }
  .choose { display: flex; gap: 14px; align-items: flex-end; flex-wrap: wrap; margin-bottom: 12px; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .choose label { display: grid; gap: 3px; }
  .confirm { border-color: var(--error-border, var(--error)); }
  .type { display: grid; gap: 4px; margin: 12px 0 8px; max-width: 360px; font-size: var(--sc-t-body); }
  .row { display: flex; gap: 8px; }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); padding: 4px 10px 4px 0; white-space: nowrap; }
  td { padding: 6px 10px 6px 0; border-top: 1px solid var(--sc-hairline); vertical-align: top; }
  tr.off td { color: var(--text-dim); }
  .why { max-width: 280px; }
  .bad { color: var(--error); }
  button.danger { color: var(--error); }
  button.link { background: none; border: none; padding: 0; color: var(--accent); }
</style>
