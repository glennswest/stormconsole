<script>
  // The Update firmware dialog (#317): for one drive or a selection of one
  // model, choose an image from each node's store or upload one, confirm, and
  // stormdrive updates them (data-serving drives one at a time, each held back
  // while a volume on it is not fully redundant). What it will not take is
  // said first: another model's image, two members of one RAID set, a member
  // of a set that is not clean (drivefirmware.js; the server checks again).
  import { postJson, get } from '../api.js'
  import { noteActivity } from '../stores.svelte.js'
  import { imageWhy, imagesFor, phrase, selectionWhy, uploadName } from '../drivefirmware.js'

  let { recs = [], images = [], guard = null, onclose, ondone, onreload } = $props()

  let imageName = $state('')
  let checked = $state(false)
  let step = $state('choose')
  let typed = $state('')
  let busy = $state(false)
  let error = $state('')
  let said = $state('')
  let file = $state(null)

  const sel = $derived(selectionWhy(recs))
  const offered = $derived(imagesFor(recs, images))
  const image = $derived([...offered.fits, ...offered.unnamed].find((i) => i.name === imageName) || null)
  const imgWhy = $derived(imageWhy(recs, image, checked))
  const word = $derived(phrase(recs))
  const nodes = $derived([...new Set(recs.map((r) => r.node))])
  const allowed = $derived(!!guard?.allowed)
  let seeded = false
  $effect(() => {
    if (seeded || !offered.fits.length) return
    seeded = true
    imageName = offered.fits.find((i) => !i.missing.length)?.name || ''
  })

  const size = (n) => (n >= 1048576 ? `${(n / 1048576).toFixed(1)} MiB` : n >= 1024 ? `${Math.round(n / 1024)} KiB` : `${n} B`)

  async function upload() {
    if (!file) return
    busy = true
    error = ''
    said = ''
    try {
      const name = uploadName(recs[0].model, file.name)
      const path = (n) => `/api/plugins/drive/firmware/images/${encodeURIComponent(n)}/${encodeURIComponent(name)}`
      const g = (await get(`/api/v1/console/guard?method=PUT&path=${encodeURIComponent(path(nodes[0]))}`)).guard
      if (!g?.allowed) throw new Error(`${g?.what || 'store a firmware image'}: ${g?.reason || 'not allowed'}`)
      const data = await file.arrayBuffer()
      for (const n of nodes) {
        const resp = await fetch(path(n), { method: 'PUT', headers: { 'X-Storm-Confirm': g.confirm, 'Content-Type': 'application/octet-stream' }, body: data })
        const body = await resp.json().catch(() => ({}))
        if (!resp.ok) throw new Error(`${n}: ${body.error || `${resp.status} ${resp.statusText}`}`)
      }
      said = `${name} stored on ${nodes.join(', ')} (${size(data.byteLength)})`
      noteActivity({ reason: 'Firmware', message: said, source: 'drives' })
      await onreload?.()
      imageName = name
      file = null
    } catch (e) {
      error = e.message
    }
    busy = false
  }

  async function start() {
    busy = true
    error = ''
    try {
      const body = { drives: recs.map((r) => ({ node: r.node, id: r.id })), image: imageName, confirm: typed.trim(), image_checked: !image?.model && checked }
      const r = await postJson('/api/plugins/drive/firmware', body, 'POST', { 'X-Storm-Confirm': guard?.confirm || 'firmware' })
      for (const o of r.results || []) {
        noteActivity({ reason: 'Firmware', message: o.ok ? `started on ${o.drives.length} on ${o.node}: ${o.image}` : `refused on ${o.node}: ${o.error}`, source: `drives/${o.node}`, warning: !o.ok })
      }
      const bad = (r.results || []).filter((o) => !o.ok)
      if (bad.length) error = bad.map((o) => `${o.node}: ${o.error}`).join('; ')
      else ondone?.(r.results)
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
  <div class="dialog" role="dialog" aria-modal="true" aria-label="Update firmware" data-testid="firmware-dialog">
    <header>
      <h2>Update firmware: {recs.length === 1 ? recs[0].name : `${recs.length} drives`}</h2>
      <button class="x" aria-label="Close" disabled={busy} onclick={() => onclose?.()}>×</button>
    </header>
    {#if !allowed}
      <p class="note" data-testid="firmware-refused">You can see the firmware here, and not update it. {guard?.reason || 'Asking whether you may…'}</p>
    {/if}
    {#if error}<p class="error" data-testid="firmware-error">{error}</p>{/if}
    {#if said}<p class="ok" data-testid="firmware-said">{said}</p>{/if}

    <div class="body">
      <div class="rows">
        {#each recs as r (r.node + r.id)}
          <div class="row" data-testid="firmware-drive" data-drive={r.name}>
            <span class="name">{r.name}</span>
            <span class="facts">{r.node} · {r.vendor ? `${r.vendor} ` : ''}{r.model || 'no model'} · {r.serial}{r.set ? ` · RAID set ${r.set} (${r.setState})` : ''}</span>
            <span class="ver">now <b data-testid="firmware-version">{r.version || '—'}</b></span>
            {#if r.whyNot}<span class="why">{r.whyNot}</span>{/if}
          </div>
        {/each}
      </div>
      {#if sel}
        <p class="refusal" data-testid="firmware-selection-why">{sel}</p>
      {:else if step === 'choose'}
        <h3>Image for {recs[0].model}</h3>
        {#if !offered.fits.length && !offered.unnamed.length}
          <p class="dim" data-testid="firmware-no-image">No image for {recs[0].model} in {nodes.join(' or ')}'s store. Upload one below.</p>
        {/if}
        <div class="rows">
          {#each [...offered.fits, ...offered.unnamed] as i (i.name)}
            <label class="row" data-testid="firmware-image" data-image={i.name}>
              <input type="radio" name="img" value={i.name} bind:group={imageName} disabled={!allowed} />
              <span class="mono">{i.name}</span>
              <span class="facts">{i.size ? size(i.size) : ''}{i.sha256 ? ` · sha256 ${String(i.sha256).slice(0, 12)}…` : ''}{i.model ? '' : ' · names no model'}{i.missing.length ? ` · not on ${i.missing.join(', ')}` : ''}</span>
            </label>
          {/each}
        </div>
        {#if image && !image.model}
          <label class="check"><input type="checkbox" bind:checked={checked} data-testid="firmware-checked" /> I checked {image.name} is firmware for {recs[0].model}: stormdrive does not check (stormdrive#126)</label>
        {/if}
        <div class="upload">
          <label>Upload an image for {recs[0].model}
            <input type="file" disabled={!allowed || busy} onchange={(e) => (file = e.currentTarget.files?.[0] || null)} data-testid="firmware-file" />
          </label>
          <button disabled={!file || busy || !allowed} onclick={upload} data-testid="firmware-upload">Upload to {nodes.join(', ')}</button>
          {#if file}<span class="dim">stored as {(() => { try { return uploadName(recs[0].model, file.name) } catch (e) { return e.message } })()}</span>{/if}
        </div>
        {#if image && imgWhy}<p class="why block" data-testid="firmware-image-why">{imgWhy}</p>{/if}
      {:else}
        <p>Each drive takes <span class="mono">{imageName}</span> and resets once, when the image activates. stormdrive updates a drive that holds data one at a time per node, and waits while a volume on it is not fully redundant. A drive's firmware cannot be put back from here.</p>
        <ul class="lines" data-testid="firmware-lines">
          {#each recs as r (r.node + r.id)}<li><strong>{r.node} · {r.name}</strong> {r.version} → {imageName}</li>{/each}
        </ul>
        <label class="type"><span>Type <span class="mono">{word}</span> to confirm</span>
          <input bind:value={typed} aria-label="Confirmation" autocomplete="off" />
        </label>
      {/if}
    </div>
    <footer>
      {#if step === 'choose'}
        <button onclick={() => onclose?.()}>Cancel</button>
        <button class="danger" disabled={!!sel || !image || !!imgWhy || !allowed || busy} onclick={() => { step = 'confirm'; typed = '' }} data-testid="firmware-next">Next: confirm</button>
      {:else}
        <button disabled={busy} onclick={() => (step = 'choose')}>Back</button>
        <button class="danger" disabled={busy || typed.trim() !== word} onclick={start} data-testid="firmware-go">Update</button>
      {/if}
    </footer>
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; z-index: 90; background: rgb(0 0 0 / 0.45); display: grid; place-items: start center; padding: 40px 8px; overflow-y: auto; }
  .dialog { width: min(760px, 100%); background: var(--panel); border: 1px solid var(--border-strong); border-radius: var(--radius); box-shadow: 0 12px 40px rgb(0 0 0 / 0.35); display: grid; }
  header { display: flex; align-items: center; gap: 10px; padding: 12px 16px; border-bottom: 1px solid var(--border); }
  h2 { font-size: 15px; margin: 0; flex: 1; }
  h3 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 12px 0 4px; }
  .x { background: none; border: none; font-size: 20px; line-height: 1; padding: 0 4px; color: var(--text-dim); }
  .body { padding: 6px 16px 12px; max-height: calc(100vh - 220px); overflow-y: auto; font-size: var(--sc-t-body); }
  .rows { display: grid; gap: 2px; }
  .row { display: flex; flex-wrap: wrap; gap: 4px 12px; align-items: center; padding: 6px 0; border-top: 1px solid var(--sc-hairline); }
  .name { font-weight: 600; min-width: 60px; }
  .facts { color: var(--text-dim); font-size: var(--sc-t-meta); flex: 1 1 220px; overflow-wrap: anywhere; }
  .ver { font-size: var(--sc-t-meta); }
  .why { flex-basis: 100%; font-size: var(--sc-t-meta); color: var(--warn-strong); overflow-wrap: anywhere; }
  .why.block { margin: 8px 0 0; }
  .check { display: flex; gap: 8px; align-items: flex-start; margin: 8px 0; font-size: var(--sc-t-meta); }
  .upload { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; margin: 12px 0 0; font-size: var(--sc-t-meta); }
  .upload label { display: grid; gap: 4px; }
  .refusal { margin: 10px 0; padding: 8px 12px; border-left: 3px solid var(--warn-strong); background: var(--panel-raised); overflow-wrap: anywhere; }
  .note { margin: 8px 16px 0; padding: 7px 12px; font-size: var(--sc-t-meta); color: var(--text-dim); border-left: 3px solid var(--accent); background: var(--panel-raised); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .error { color: var(--error); margin: 8px 16px 0; overflow-wrap: anywhere; }
  .ok { color: var(--ok, var(--text)); margin: 8px 16px 0; font-size: var(--sc-t-meta); }
  .lines { margin: 8px 0; padding-left: 18px; display: grid; gap: 4px; }
  footer { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; justify-content: flex-end; padding: 10px 16px; border-top: 1px solid var(--border); }
  .type { display: grid; gap: 4px; margin: 12px 0 0; max-width: 360px; }
  .mono { font-family: var(--mono); overflow-wrap: anywhere; }
  button.danger { color: var(--error); }
</style>
