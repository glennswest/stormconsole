<script>
  // The Replace dialog (#317): pick the drive that takes the old one's place,
  // read what will happen, type the old drive's word, and it is done by what
  // holds the old drive (drivereplace.js says which): the engine's live
  // replace for a RAID member, a join and a drain for a slab member, a record
  // for a free drive. The system drive is refused, with why.
  import { call, get } from '../api.js'
  import { noteActivity } from '../stores.svelte.js'
  import { formatBytes } from '../drivemap.js'
  import { KIND_WORDS, asked, candidates, kindOf, plan } from '../drivereplace.js'

  let { old, records = [], ctx = {}, onclose, ondone, onformat } = $props()

  const kind = $derived(kindOf(old))
  const first = $derived(plan(old, null, ctx))
  const list = $derived(candidates(old, records, ctx))
  let pickedId = $state('')
  let step = $state('choose')
  let typed = $state('')
  let busy = $state(false)
  let error = $state('')
  let guard = $state(null)
  let seeded = false
  $effect(() => {
    if (seeded || !list.length) return
    seeded = true
    pickedId = list.find((x) => x.p.ok)?.d.id || ''
  })
  const neu = $derived(list.find((x) => x.d.id === pickedId)?.d || null)
  const p = $derived(neu ? plan(old, neu, ctx) : null)
  const name = (d) => d?.dev?.replace(/^\/dev\//, '') || d?.serial || d?.id
  // What to type: the engine's guard word for a RAID member (it checks it), else the old drive's serial.
  const word = $derived(p?.call?.confirm ? guard?.confirm || '' : old.serial || name(old))

  async function next() {
    error = ''
    if (p?.call?.confirm) {
      try {
        const g = await get(`/api/v1/console/guard?method=POST&path=${encodeURIComponent(p.call.path)}`)
        guard = g.guard
        if (!guard?.allowed) return (error = `${guard?.what || 'replace a set member'}: ${guard?.reason || 'not allowed'}`)
      } catch (e) {
        return (error = e.message)
      }
    }
    typed = ''
    step = 'confirm'
  }

  async function run() {
    busy = true
    error = ''
    const what = `${name(old)} → ${name(neu)}`
    try {
      if (p.join) {
        await call(p.join.method, p.join.path)
        noteActivity({ reason: 'Replace', message: `${name(neu)} joined the fleet`, source: `drives/${old.node}` })
      }
      const r = await call(p.call.method, p.call.path, p.call.confirm ? typed.trim() : undefined, p.call.body)
      noteActivity({ reason: 'Replace', message: r?.note || `started, ${what}`, source: `drives/${old.node}` })
      ondone?.(asked(p, old, neu, Math.floor(Date.now() / 1000)), r)
    } catch (e) {
      if (e.message !== 'cancelled') {
        error = e.message
        noteActivity({ reason: 'Replace', message: `refused, ${what}: ${e.message}`, source: `drives/${old.node}`, warning: true })
      }
    }
    busy = false
  }
  function onkey(e) {
    if (e.key === 'Escape' && !busy) onclose?.()
  }
</script>

<svelte:window onkeydown={onkey} />

<div class="scrim">
  <div class="dialog" role="dialog" aria-modal="true" aria-label="Replace {name(old)}" data-testid="replace-dialog">
    <header>
      <h2>Replace {name(old)}</h2>
      <span class="badge" class:system={kind === 'system'} data-testid="replace-kind">{KIND_WORDS[kind]}</span>
      <button class="x" aria-label="Close" disabled={busy} onclick={() => onclose?.()}>×</button>
    </header>
    <p class="facts pad">{old.node} · {old.serial}{old.bay !== null && old.bay !== undefined ? ` · bay ${old.bay}` : ''} · {old.capacity ? formatBytes(old.capacity) : '—'}{old.set ? ` · RAID set ${old.set}` : ''}</p>
    {#if error}<p class="error" data-testid="replace-error">{error}</p>{/if}

    {#if !first.ok}
      <p class="refusal" data-testid="replace-refused">{first.why}</p>
      <footer><button onclick={() => onclose?.()}>Close</button></footer>
    {:else if step === 'choose'}
      <div class="body">
        <h3>Replace it with</h3>
        {#if !list.length}
          <p class="refusal" data-testid="replace-none">No free drive in {old.node}. Insert one, and it appears here once its stormdrive sees it.</p>
        {/if}
        <div class="rows">
          {#each list as x (x.d.id)}
            <label class="row" class:off={!x.p.ok} data-testid="replace-candidate" data-drive={name(x.d)}>
              <input type="radio" name="neu" value={x.d.id} bind:group={pickedId} disabled={!x.p.ok} />
              <span class="name">{name(x.d)}</span>
              <span class="facts">{x.d.serial}{x.d.bay !== null && x.d.bay !== undefined ? ` · bay ${x.d.bay}` : ''} · {x.d.capacity ? formatBytes(x.d.capacity) : '—'}{x.d.shelfId === old.shelfId ? ' · same shelf' : ''}</span>
              {#if !x.p.ok}
                <span class="why" data-testid="replace-why">{x.p.why}</span>
                {#if x.p.format && onformat}
                  <button class="small-btn" onclick={(e) => { e.preventDefault(); onformat(x.d, x.p.format) }} data-testid="replace-format-first">Format {name(x.d)}…</button>
                {/if}
              {/if}
            </label>
          {/each}
        </div>
        {#if p?.ok}
          <h3>What happens</h3>
          <ol class="steps" data-testid="replace-steps">
            {#each p.steps as s}<li>{s}</li>{/each}
          </ol>
          {#if p.note}<p class="dim">{p.note}</p>{/if}
        {/if}
      </div>
      <footer>
        <button onclick={() => onclose?.()}>Cancel</button>
        <button class="primary" disabled={!p?.ok} onclick={next} data-testid="replace-next">Next: confirm</button>
      </footer>
    {:else}
      <div class="body">
        <p><strong>{name(old)} → {name(neu)}</strong> on {old.node}.</p>
        <ol class="steps">{#each p.steps as s}<li>{s}</li>{/each}</ol>
        <label class="type"><span>Type <span class="mono" data-testid="replace-word">{word}</span> to confirm</span>
          <input bind:value={typed} aria-label="Confirmation" autocomplete="off" />
        </label>
      </div>
      <footer>
        <button disabled={busy} onclick={() => (step = 'choose')}>Back</button>
        <button class="primary" disabled={busy || !word || typed.trim() !== word} onclick={run} data-testid="replace-go">Replace</button>
      </footer>
    {/if}
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; z-index: 90; background: rgb(0 0 0 / 0.45); display: grid; place-items: start center; padding: 40px 8px; overflow-y: auto; }
  .dialog { width: min(760px, 100%); background: var(--panel); border: 1px solid var(--border-strong); border-radius: var(--radius); box-shadow: 0 12px 40px rgb(0 0 0 / 0.35); display: grid; }
  header { display: flex; align-items: center; gap: 10px; padding: 12px 16px; border-bottom: 1px solid var(--border); }
  h2 { font-size: 15px; margin: 0; }
  h3 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 12px 0 4px; }
  .x { background: none; border: none; font-size: 20px; line-height: 1; padding: 0 4px; color: var(--text-dim); margin-left: auto; }
  .pad { padding: 8px 16px 0; margin: 0; }
  .body { padding: 4px 16px 12px; max-height: calc(100vh - 240px); overflow-y: auto; font-size: var(--sc-t-body); }
  .rows { display: grid; gap: 2px; }
  .row { display: flex; flex-wrap: wrap; gap: 4px 12px; align-items: center; padding: 7px 0; border-top: 1px solid var(--sc-hairline); }
  .row.off { color: var(--text-dim); }
  .name { font-weight: 600; min-width: 60px; }
  .facts { color: var(--text-dim); font-size: var(--sc-t-meta); flex: 1 1 220px; }
  .why { flex-basis: 100%; font-size: var(--sc-t-meta); color: var(--warn-strong); padding-left: 24px; overflow-wrap: anywhere; }
  .small-btn { margin-left: 24px; font-size: var(--sc-t-meta); padding: 2px 8px; }
  .badge { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; padding: 1px 6px; border-radius: 999px; border: 1px solid var(--warn-strong); color: var(--warn-strong); }
  .badge.system { border-color: var(--error); color: var(--error); }
  .steps { margin: 4px 0; padding-left: 20px; display: grid; gap: 4px; }
  .refusal { margin: 10px 16px; padding: 8px 12px; border-left: 3px solid var(--warn-strong); background: var(--panel-raised); font-size: var(--sc-t-body); overflow-wrap: anywhere; }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .error { color: var(--error); margin: 8px 16px 0; font-size: var(--sc-t-body); overflow-wrap: anywhere; }
  footer { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; justify-content: flex-end; padding: 10px 16px; border-top: 1px solid var(--border); }
  .type { display: grid; gap: 4px; margin: 12px 0 0; max-width: 360px; }
  .mono { font-family: var(--mono); }
</style>
