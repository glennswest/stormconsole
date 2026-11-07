<script>
  // The Format dialog (#308): choose the drives and what each becomes, then
  // confirm, naming each drive's result.
  //
  // Every drive the format page knows is listed with what it is now and
  // whether it is in use. One in use (the system drive, a slab, a RAID
  // member) is shown and cannot be ticked, with the reason. A drive's
  // choices are only what its `supports` offers, and the default is its own
  // plan (plannedFormat: 4096 + PI type 1 on forge1). The drives go in one
  // request per node and format (stormdrive's `POST /api/v1/format`), as the
  // person asking (#82), with the phrase typed on top.
  import { postJson } from '../api.js'
  import { noteActivity } from '../stores.svelte.js'
  import { formatBytes } from '../drivemap.js'
  import {
    badgeOf, confirmLine, currentText, defaultOption, formatText, inUse, isRunning, optionKey, options, parseKey,
    phrase, piNote, planFor, requestFor, resultText, selectable,
  } from '../driveformat.js'

  let { records = [], picked = [], guard = null, onclose, ondone } = $props()

  const key = (r) => `${r.node}/${r.id}`
  let ticked = $state({})
  let choice = $state({})
  let all = $state('')
  let step = $state('choose')
  let typed = $state('')
  let busy = $state(false)
  let error = $state('')
  let seeded = false

  // Once per opening: the drives asked for, each on its own plan.
  $effect(() => {
    if (seeded || !records.length) return
    seeded = true
    const t = {}
    const c = {}
    for (const r of records) {
      c[key(r)] = optionKey(defaultOption(r))
      if (picked.includes(key(r)) && selectable(r)) t[key(r)] = true
    }
    ticked = t
    choice = c
  })

  const nodes = $derived([...new Set(records.map((r) => r.node))])
  const chosen = $derived(records.filter((r) => ticked[key(r)] && selectable(r)))
  const word = $derived(phrase(chosen))
  const allowed = $derived(!!guard?.allowed)
  // The formats offered on every chosen drive, for "the same on all".
  const shared = $derived.by(() => {
    if (!chosen.length) return []
    const lists = chosen.map((r) => options(r).map(optionKey))
    return lists[0].filter((k) => lists.every((l) => l.includes(k)))
  })

  function setAll(k) {
    if (!k) return
    const c = { ...choice }
    for (const r of chosen) if (options(r).some((o) => optionKey(o) === k)) c[key(r)] = k
    choice = c
  }
  function why(r) {
    if (isRunning(r)) return 'a format is running on it'
    return r.whyNot || ''
  }
  const fallbackText = (p) => p.fallbacks.map((f) => formatText(f.blockSize, f.protection)).join(', then ')

  async function start() {
    busy = true
    error = ''
    try {
      const body = requestFor(chosen, choice, typed.trim())
      const r = await postJson('/api/plugins/drive/format', body, 'POST', { 'X-Storm-Confirm': guard?.confirm || 'format' })
      const results = r.results || []
      for (const o of results) {
        const what = `${o.drives.length} ${o.drives.length === 1 ? 'drive' : 'drives'} on ${o.node}: ${formatText(o.block_size, o.protection)}`
        noteActivity({ reason: 'Format', message: o.ok ? `started, ${what}` : `refused, ${what}: ${o.error}`, source: `drives/${o.node}`, warning: !o.ok })
      }
      ondone?.(results)
    } catch (e) {
      error = e.message
    }
    busy = false
  }
  function onkey(e) {
    if (e.key === 'Escape' && !busy) onclose?.()
  }
</script>

<svelte:window onkeydown={onkey} />

<div class="scrim">
  <div class="dialog" role="dialog" aria-modal="true" aria-label="Format drives">
    <header>
      <h2>{step === 'choose' ? 'Format drives' : `Format ${chosen.length} ${chosen.length === 1 ? 'drive' : 'drives'}: their data goes`}</h2>
      <button class="x" aria-label="Close" disabled={busy} onclick={() => onclose?.()}>×</button>
    </header>

    {#if !allowed}
      <p class="note" data-testid="dialog-refused">You can see the drives here, and not format them. {guard?.reason || 'Asking whether you may…'}</p>
    {/if}
    {#if error}<p class="error" data-testid="dialog-error">{error}</p>{/if}

    {#if step === 'choose'}
      <div class="body">
        {#each nodes as n (n)}
          <h3>{n}</h3>
          <div class="rows">
            {#each records.filter((r) => r.node === n) as r (key(r))}
              {@const opts = options(r)}
              {@const off = !selectable(r)}
              {@const badge = badgeOf(r)}
              <div class="row" class:off data-testid="dialog-drive" data-drive={r.name}>
                <label class="pick">
                  <input type="checkbox" aria-label="Choose {r.name}" disabled={off || !allowed} bind:checked={ticked[key(r)]} />
                  <span class="name">{r.name}</span>
                  {#if badge}<span class="badge" class:system={r.use?.use === 'system'}>{badge}</span>{/if}
                </label>
                <span class="facts">
                  {r.model}{r.bay !== null && r.bay !== undefined ? ` · bay ${r.bay}` : ''} · {r.capacity ? formatBytes(r.capacity) : '—'}
                  · now {currentText(r)}
                </span>
                {#if off}
                  <span class="why" data-testid="dialog-why">{why(r)}</span>
                {:else}
                  <label class="to">becomes
                    <select aria-label="Format for {r.name}" bind:value={choice[key(r)]} disabled={!allowed}>
                      {#each opts as o (optionKey(o))}
                        <option value={optionKey(o)}>{resultText(o.block_size, o.protection)}{o.basis === 'try' ? ' (a try)' : ''}{planFor(r, o).onPlan ? ' · its plan' : ''}</option>
                      {/each}
                    </select>
                  </label>
                  {#if piNote(r)}<span class="dim small">{piNote(r)}</span>{/if}
                  {#if r.use?.holds}<span class="warn small">holds {r.use.holds}: it goes</span>{/if}
                {/if}
              </div>
            {/each}
          </div>
        {/each}
      </div>
      <footer>
        {#if chosen.length > 1 && shared.length}
          <label class="all">Same on all
            <select bind:value={all} onchange={() => setAll(all)} aria-label="Same format on all">
              <option value="">—</option>
              {#each shared as k (k)}{@const o = parseKey(k)}<option value={k}>{resultText(o.block_size, o.protection)}</option>{/each}
            </select>
          </label>
        {/if}
        <span class="count">{chosen.length} chosen</span>
        <button onclick={() => onclose?.()}>Cancel</button>
        <button class="danger" disabled={!chosen.length || !allowed} onclick={() => { step = 'confirm'; typed = '' }} data-testid="dialog-next">Next: confirm</button>
      </footer>
    {:else}
      <div class="body">
        <p>Everything on these drives is destroyed and cannot be recovered. A large drive takes most of a day, and a format cannot be cancelled. It is done as you.</p>
        <ul class="lines" data-testid="dialog-lines">
          {#each chosen as r (key(r))}
            {@const o = planFor(r, parseKey(choice[key(r)]))}
            <li>
              <strong>{r.node} · {confirmLine(r, o)}</strong>
              <span class="dim">{r.serial} · now {currentText(r)}{o.fallbacks.length ? ` · if the drive refuses: ${fallbackText(o)}` : ''}</span>
            </li>
          {/each}
        </ul>
        <label class="type"><span>Type <span class="mono">{word}</span> to confirm</span>
          <input bind:value={typed} aria-label="Confirmation" autocomplete="off" />
        </label>
      </div>
      <footer>
        <button disabled={busy} onclick={() => (step = 'choose')}>Back</button>
        <button class="danger" disabled={busy || typed.trim() !== word} onclick={start} data-testid="dialog-format">Format</button>
      </footer>
    {/if}
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; z-index: 90; background: rgb(0 0 0 / 0.45); display: grid; place-items: start center; padding: 40px 8px; overflow-y: auto; }
  .dialog { width: min(820px, 100%); background: var(--panel); border: 1px solid var(--border-strong); border-radius: var(--radius); box-shadow: 0 12px 40px rgb(0 0 0 / 0.35); display: grid; }
  header { display: flex; align-items: center; gap: 10px; padding: 12px 16px; border-bottom: 1px solid var(--border); }
  h2 { font-size: 15px; margin: 0; flex: 1; }
  h3 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 10px 0 4px; }
  .x { background: none; border: none; font-size: 20px; line-height: 1; padding: 0 4px; color: var(--text-dim); }
  .body { padding: 8px 16px 12px; max-height: calc(100vh - 220px); overflow-y: auto; }
  .body p { font-size: var(--sc-t-body); }
  .rows { display: grid; gap: 2px; }
  .row { display: flex; flex-wrap: wrap; gap: 4px 14px; align-items: center; padding: 7px 0; border-top: 1px solid var(--sc-hairline); font-size: var(--sc-t-body); }
  .row.off { color: var(--text-dim); }
  .pick { display: inline-flex; gap: 8px; align-items: center; min-width: 150px; }
  .name { font-weight: 600; }
  .facts { color: var(--text-dim); font-size: var(--sc-t-meta); flex: 1 1 220px; }
  .to { display: inline-flex; gap: 6px; align-items: center; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .why { flex-basis: 100%; font-size: var(--sc-t-meta); color: var(--warn-strong); padding-left: 26px; }
  .badge { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; padding: 1px 6px; border-radius: 999px; border: 1px solid var(--warn-strong); color: var(--warn-strong); }
  .badge.system { border-color: var(--error); color: var(--error); }
  .small { font-size: var(--sc-t-meta); flex-basis: 100%; padding-left: 26px; }
  .dim { color: var(--text-dim); }
  .warn { color: var(--warn-strong); }
  .error { color: var(--error); margin: 8px 16px 0; font-size: var(--sc-t-body); }
  .note { margin: 8px 16px 0; padding: 7px 12px; font-size: var(--sc-t-meta); color: var(--text-dim); border-left: 3px solid var(--accent); background: var(--panel-raised); }
  footer { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; justify-content: flex-end; padding: 10px 16px; border-top: 1px solid var(--border); }
  .all { display: inline-flex; gap: 6px; align-items: center; font-size: var(--sc-t-meta); color: var(--text-dim); margin-right: auto; }
  .count { font-size: var(--sc-t-meta); color: var(--text-dim); }
  .lines { margin: 8px 0; padding-left: 18px; display: grid; gap: 6px; font-size: var(--sc-t-body); }
  .lines li span { display: block; font-size: var(--sc-t-meta); }
  .type { display: grid; gap: 4px; margin: 12px 0 0; max-width: 360px; font-size: var(--sc-t-body); }
  .mono { font-family: var(--mono); }
  button.danger { color: var(--error); }
</style>
