<script>
  // Drives the engine holds and has not been told to use (stormblock#154): what
  // each holds, and a format that only a person starts, one drive at a time,
  // having read the risk and typed the drive's serial. Then, because a new
  // slab sits empty until writes land on it, a rebalance: a dry run first.
  //
  // Two locks, both on the server. The console's storage gate takes the word in
  // X-Storm-Confirm and asks the apiserver as the viewer (#82); the engine takes
  // the same word in the body and asks again (#274, #382). Without the word in
  // the body the engine answers 412 naming the risk and touches nothing, which
  // is how the page gets the sentence it shows.
  import { onDestroy } from 'svelte'
  import { feed } from '../stores.svelte.js'
  import { call } from '../api.js'
  import { engineDrives, riskText, planSentence, statusLine } from '../driveoffer.js'

  let { asking = $bindable(null) } = $props()

  const REBALANCE = '/api/plugins/sb/proxy/api/v1/slabs/rebalance'

  const offered = $derived(engineDrives(feed.components).filter((d) => d.offered))

  let risk = $state('')
  let word = $state('')
  let typed = $state('')
  let error = $state('')
  let busy = $state(false)
  let done = $state('')
  let asked = null

  async function send(path, typedWord, body) {
    const resp = await fetch(path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Storm-Confirm': typedWord },
      body: JSON.stringify(body),
    })
    return { status: resp.status, ok: resp.ok, data: await resp.json().catch(() => ({})) }
  }

  // Ask for the risk: no `confirm` in the body, so nothing can happen.
  async function readRisk(d) {
    risk = ''
    word = ''
    typed = ''
    error = ''
    done = ''
    const r = await send(d.action.path, d.confirm, {})
    if (r.status === 412) {
      risk = riskText(r.data.error)
      word = r.data.confirm
    } else {
      error = r.data.error || `the engine answered ${r.status}`
    }
  }
  $effect(() => {
    // Once per drive asked about.
    if (asking && asking !== asked && asking.id !== asked?.id) {
      asked = asking
      if (asking.action) readRisk(asking)
      else error = 'Only a storage-admin may format a drive.'
    }
    if (!asking) asked = null
  })

  function cancel() {
    asking = null
    risk = ''
    typed = ''
    error = ''
  }

  async function format() {
    if (busy || typed.trim() !== word) return
    busy = true
    error = ''
    const d = asking
    try {
      const r = await send(d.action.path, typed.trim(), { confirm: typed.trim() })
      if (!r.ok) {
        error = r.data.error || `the engine answered ${r.status}`
        // A format can outlast the proxy's 30 s, and it goes on without it.
        if (r.status >= 500) error += ' The format may still be running: if this drive leaves the list above, it finished.'
        return
      }
      done = `${d.path || d.label} is a data slab now.`
      asking = null
      risk = ''
      typed = ''
      planRebalance()
    } catch (e) {
      error = `no answer from the engine: ${e.message}. The drive's state below is the engine's; check it before trying again.`
    } finally {
      busy = false
    }
  }

  // --- the rebalance
  let plan = $state(null)
  let planError = $state('')
  let planning = $state(false)
  let status = $state(null)
  let timer = null

  async function rebalance(dry) {
    const resp = await fetch(REBALANCE, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ dry_run: dry }),
    })
    const data = await resp.json().catch(() => ({}))
    if (!resp.ok) throw new Error(data.error || `the engine answered ${resp.status}`)
    return data
  }

  async function planRebalance() {
    planError = ''
    planning = true
    try {
      plan = (await rebalance(true)).plan
    } catch (e) {
      planError = e.message
    } finally {
      planning = false
    }
  }

  async function runRebalance() {
    planError = ''
    try {
      const r = await rebalance(false)
      status = r.status || null
      plan = r.status ? null : r.plan
      watch()
    } catch (e) {
      planError = e.message
    }
  }

  async function stopRebalance() {
    try {
      await call('DELETE', REBALANCE)
      watch()
    } catch (e) {
      planError = e.message
    }
  }

  // Also when the page opens, so a rebalance someone started is seen. Quiet on
  // 404 (none has run) and on an engine from before it.
  async function watch() {
    clearTimeout(timer)
    let every = 15000
    try {
      const resp = await fetch(REBALANCE)
      if (resp.ok) {
        status = await resp.json()
        if (statusLine(status).running) every = 2000
      }
    } catch {
      // The next look says.
    }
    timer = setTimeout(watch, every)
  }
  watch()
  onDestroy(() => clearTimeout(timer))

  const progress = $derived(statusLine(status))
  const recent = $derived(status && !progress.running && status.finished_at && Date.now() / 1000 - status.finished_at < 900)
</script>

{#if offered.length}
  <section class="offers" aria-label="Offered drives">
    <h2>Offered drives <span class="n">{offered.length}</span></h2>
    <p class="dim">This node's engine holds these and is not using them. Nothing formats a drive on its own: a test may be using it, and a power cut during a format leaves it unusable.</p>
    <table>
      <thead><tr><th>Drive</th><th>Serial</th><th>Model</th><th>Size</th><th>What it holds</th><th></th></tr></thead>
      <tbody>
        {#each offered as d (d.id)}
          <tr class:sel={asking?.id === d.id}>
            <td class="mono">{d.path || d.label}</td>
            <td class="mono">{d.serial || '—'}</td>
            <td>{d.model || '—'}</td>
            <td class="num">{d.capacity}</td>
            <td class:warn={d.state === 'foreign'}>{d.sentence}</td>
            <td class="act">
              {#if d.action}
                <button class="danger" disabled={busy} onclick={() => (asking = d)}>Format as a slab…</button>
              {:else}
                <span class="dim small">a storage-admin's to format</span>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </section>
{/if}

{#if asking}
  <section class="offers ask" aria-label="Format a drive">
    <h2>Format {asking.path || asking.label}</h2>
    {#if error}<p class="bad">{error}</p>{/if}
    {#if risk}
      <p class="risk">{risk}</p>
      <label class="type">
        <span>Type <strong class="mono">{word}</strong> to format this drive</span>
        <input bind:value={typed} aria-label="The drive's name, typed" autocomplete="off" spellcheck="false" disabled={busy} />
      </label>
    {/if}
    <div class="row">
      {#if risk}
        <button class="danger" disabled={busy || typed.trim() !== word} onclick={format}>{busy ? 'Formatting…' : 'Format this drive'}</button>
      {/if}
      <button disabled={busy} onclick={cancel}>Cancel</button>
    </div>
  </section>
{/if}

{#if done || plan || planning || planError || progress.running || recent}
  <section class="offers" aria-label="Rebalance">
    {#if done}<p class="good">{done}</p>{/if}
    {#if planning}<p class="dim">Planning a rebalance onto it, moving nothing…</p>{/if}
    {#if planError}<p class="bad">{planError}</p>{/if}
    {#if plan}
      <h2>Rebalance: a dry run</h2>
      <p>{planSentence(plan)} A new slab otherwise fills only as new writes land on it.</p>
      <div class="row">
        {#if plan.moves}<button class="primary" onclick={runRebalance}>Run this rebalance</button>{/if}
        <button onclick={() => { plan = null; done = '' }}>{plan.moves ? 'Not now' : 'Close'}</button>
      </div>
    {/if}
    {#if progress.running || recent}
      <h2>Rebalance</h2>
      <p class:bad={progress.failed}>{progress.line}</p>
      {#each status.errors || [] as e}<p class="bad small">{e}</p>{/each}
      {#if status.planned}<div class="track"><span style="width: {Math.min(100, (status.moved / status.planned) * 100)}%"></span></div>{/if}
      {#if progress.running}<button onclick={stopRebalance}>Stop after the move under way</button>{/if}
    {/if}
  </section>
{/if}

<style>
  .offers { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 12px var(--sc-row-px); margin-bottom: 12px; }
  .ask { border-color: var(--error); }
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0 0 6px; }
  .n { font-variant-numeric: tabular-nums; }
  p { margin: 0 0 8px; }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
  .warn { color: var(--warn-strong); }
  .bad { color: var(--error); }
  .good { color: var(--ok); }
  .risk { font-weight: 600; }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); padding: 6px 10px; border-bottom: 1px solid var(--border); }
  td { padding: 6px 10px; vertical-align: middle; }
  tr + tr > td { border-top: 1px solid var(--sc-hairline); }
  tr.sel td { background: var(--nav-hover); }
  td.num { font-variant-numeric: tabular-nums; font-family: var(--mono); font-size: var(--sc-t-meta); white-space: nowrap; }
  td.act { text-align: right; white-space: nowrap; }
  .type { display: grid; gap: 4px; max-width: 420px; margin-bottom: 10px; }
  button.danger { color: var(--error); }
  .row { display: flex; gap: 8px; flex-wrap: wrap; }
  .track { height: 6px; background: var(--sc-hairline); border-radius: 3px; overflow: hidden; margin-bottom: 8px; }
  .track span { display: block; height: 100%; background: var(--accent); }
</style>
