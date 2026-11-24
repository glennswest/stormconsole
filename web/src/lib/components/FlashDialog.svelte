<script>
  // Flash a SAS card (#376, stormdrive#166): sas3flash's job, through the
  // card's own interface. The image is a golden, one of two ways: the release
  // manifest's entry for the card's region at the version it runs (#380,
  // stormdrive#184: file, sha256 and source from the record, the sha256
  // pinning what was shown), offered first when the card carries one; or a
  // file in the card's node's firmware store with its sha256, and the URL it
  // was recorded from, typed. The request becomes a CardOperation created as the person
  // (a storage-admin), with the card's name typed; the node then checks the
  // image against the card, the system half, and I/O in flight, and its answer
  // comes back on the operation. A dry run checks all of that and writes nothing.
  import { call, get } from '../api.js'
  import { noteActivity } from '../stores.svelte.js'
  import { formatBytes } from '../drivemap.js'
  import { body, current, manifestBody, offer, requestWhy, storeHolds, storeImages } from '../hba.js'

  /// `card`: the card as /hbas serves it; `images`: the drive plugin's firmware store answer.
  let { card, images = [], storeError = '', onclose, ondone } = $props()

  const path = $derived(`/api/plugins/k8s/hbas/${encodeURIComponent(card.name)}/flash`)
  const st = $derived(card.status || {})
  const choices = $derived(storeImages(card, images))

  let region = $state('firmware')
  // 'manifest' | 'store'; '' follows the region: the manifest's entry when it has one.
  let chosen = $state('')
  let pickedName = $state('')
  let source = $state('')
  let dryRun = $state(true)
  let typed = $state('')
  let guard = $state(null)
  let guardError = $state('')
  let busy = $state(false)
  let error = $state('')

  $effect(() => {
    get(`/api/v1/console/guard?method=POST&path=${encodeURIComponent(path)}`)
      .then((g) => (guard = g.guard))
      .catch((e) => (guardError = e.message))
  })
  const picked = $derived(choices.find((i) => i.name === pickedName) || null)
  const off = $derived(offer(card, region))
  const entry = $derived(off?.flashable ? off.image : null)
  const from = $derived(chosen || (entry ? 'manifest' : 'store'))
  const holds = $derived(storeHolds(card, entry, images))
  const req = $derived(from === 'manifest' ? manifestBody(region, entry, dryRun) : body(region, picked, source, dryRun))
  const imageName = $derived(from === 'manifest' ? entry?.file || 'the manifest’s entry' : req.image.name)
  const why = $derived(requestWhy(req))
  const word = $derived(guard?.confirm || '')
  const refused = $derived(guard && !guard.allowed ? guard.reason || 'not allowed' : guardError)

  async function run() {
    busy = true
    error = ''
    try {
      const r = await call('POST', path, typed.trim(), req)
      const said = `${card.name}: ${dryRun ? 'dry run' : 'flash'} of ${region} with ${imageName} asked for (${r.operation?.name || 'operation'})`
      noteActivity({ reason: 'Flash', message: said, source: `hbas/${st.node}` })
      ondone?.(said, r)
    } catch (e) {
      if (e.message !== 'cancelled') {
        error = e.message
        noteActivity({ reason: 'Flash', message: `refused: ${e.message}`, source: `hbas/${st.node}`, warning: true })
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
  <div class="dialog" role="dialog" aria-modal="true" aria-label="Flash {card.name}" data-testid="flash-dialog">
    <header>
      <h2>Flash {st.board || st.adapter || 'SAS card'} on {st.node}</h2>
      <button class="x" aria-label="Close" disabled={busy} onclick={() => onclose?.()}>×</button>
    </header>
    <div class="body">
      <ul class="steps">
        <li>{card.name} ({st.pci}) runs firmware {st.firmwareVersion || '?'}{st.biosVersion ? `, BIOS ${st.biosVersion}` : ''}.</li>
        <li>The image is written with the card’s firmware download, read back and compared byte for byte. A read-back that differs is never reset: the card keeps running what it ran.</li>
        <li>On a match the card is reset once no I/O is in flight on its drives (up to 30 s; they pause as on a link reset). Without that moment the image runs at the card’s next reset.</li>
        <li>Refused while a drive on the card holds {st.node}’s system half, unless {st.node} is cordoned.</li>
      </ul>

      <fieldset>
        <legend>Region</legend>
        <label><input type="radio" bind:group={region} value="firmware" onchange={() => (chosen = '')} data-testid="flash-region-firmware" /> Firmware (now {current(card, 'firmware') || '?'})</label>
        <label><input type="radio" bind:group={region} value="bios" onchange={() => (chosen = '')} data-testid="flash-region-bios" /> BIOS / UEFI option ROM (now {current(card, 'bios') || '?'})</label>
      </fieldset>

      <fieldset data-testid="flash-from">
        <legend>Image</legend>
        <label><input type="radio" name="flash-from" checked={from === 'manifest'} onchange={() => (chosen = 'manifest')} disabled={!entry} data-testid="flash-from-manifest" /> The release manifest’s golden for this card</label>
        {#if entry}
          <dl class="golden" data-testid="flash-entry">
            <dt>Version</dt><dd>{entry.version} (now {off.current || '?'})</dd>
            <dt>File</dt><dd class="mono">{entry.file}</dd>
            <dt>sha256</dt><dd class="mono" data-testid="flash-entry-sha">{entry.sha256}</dd>
            <dt>Recorded from</dt><dd class="mono" data-testid="flash-entry-source">{entry.source}</dd>
            {#if entry.golden}<dt>Golden</dt><dd class="mono">{entry.golden}</dd>{/if}
          </dl>
          {#if storeError}<p class="dim" data-testid="flash-entry-store">{st.node}’s store could not be read, so whether it holds the file is not known; the node reads it beside the manifest first.</p>
          {:else}<p class:dim={holds.held} class:why={!holds.held} data-testid="flash-entry-store">{holds.text}</p>{/if}
        {:else}
          <p class="dim" data-testid="flash-no-entry">
            {#if !off}{st.node}’s stormdrive writes no firmware manifest offers (older than v0.59.0, stormdrive#184).
            {:else if off.error}The release manifest offers no {region} image: {off.error}.
            {:else}The release manifest offers no {region} image for this card at {off.current || 'its version'}.{/if}
          </p>
        {/if}
        <label><input type="radio" name="flash-from" checked={from === 'store'} onchange={() => (chosen = 'store')} data-testid="flash-from-store" /> An image in {st.node}’s firmware store, with the URL it was recorded from</label>
      </fieldset>

      {#if from === 'store'}
      <label class="field"><span>Image, from {st.node}’s firmware store</span>
        <select bind:value={pickedName} data-testid="flash-image">
          <option value="">{choices.length ? 'Choose an image…' : 'No image in the store'}</option>
          {#each choices as i (i.name)}<option value={i.name}>{i.name}{i.size ? ` · ${formatBytes(i.size)}` : ''}</option>{/each}
        </select>
      </label>
      {#if storeError}<p class="dim" data-testid="flash-store-error">{st.node}’s store could not be read: {storeError}</p>{/if}
      {#if !choices.length && !storeError}
        <p class="dim">Put the image in {st.node}’s firmware store first: <span class="mono">PUT /api/v1/firmware/images/&lt;name&gt;</span> on its stormdrive (:9092), as a storage-admin.</p>
      {/if}
      {#if picked}<p class="sha" data-testid="flash-sha">sha256 <span class="mono">{picked.sha256}</span></p>{/if}

      <label class="field"><span>Recorded from (the URL the image came from: its golden’s source)</span>
        <input bind:value={source} placeholder="https://…" autocomplete="off" data-testid="flash-source" />
      </label>
      <p class="dim">The golden is the store’s file, its sha256 and this URL, as stormdrive records it on the operation.</p>
      {/if}

      <label class="check"><input type="checkbox" bind:checked={dryRun} data-testid="flash-dry" /> Dry run: check the image and the guards, write nothing</label>

      {#if why}
        <p class="why" data-testid="flash-why">{why}.</p>
      {:else if refused}
        <p class="refusal" data-testid="flash-refused">{refused}</p>
      {:else if !guard}
        <p class="dim">Asking whether you may…</p>
      {:else}
        <label class="type"><span>Type <span class="mono" data-testid="flash-word">{word}</span> to confirm</span>
          <input bind:value={typed} aria-label="Confirmation" autocomplete="off" data-testid="flash-typed" />
        </label>
        <p class="dim">Done as you; only a storage-admin may.</p>
      {/if}
      {#if error}<p class="error" data-testid="flash-error">{error}</p>{/if}
    </div>
    <footer>
      <button disabled={busy} onclick={() => onclose?.()}>Cancel</button>
      <button class:primary={dryRun} class:danger={!dryRun} disabled={busy || !!why || !!refused || !word || typed.trim() !== word} onclick={run} data-testid="flash-go">{dryRun ? 'Dry run' : 'Flash'}</button>
    </footer>
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; z-index: 90; background: rgb(0 0 0 / 0.45); display: grid; place-items: start center; padding: 40px 8px; overflow-y: auto; }
  .dialog { width: min(660px, 100%); background: var(--panel); border: 1px solid var(--border-strong); border-radius: var(--radius); box-shadow: 0 12px 40px rgb(0 0 0 / 0.35); display: grid; }
  header { display: flex; align-items: center; gap: 10px; padding: 12px 16px; border-bottom: 1px solid var(--border); }
  h2 { font-size: 15px; margin: 0; overflow-wrap: anywhere; }
  .x { background: none; border: none; font-size: 20px; line-height: 1; padding: 0 4px; color: var(--text-dim); margin-left: auto; }
  .body { padding: 8px 16px 12px; font-size: var(--sc-t-body); display: grid; gap: 8px; }
  .steps { margin: 4px 0; padding-left: 20px; display: grid; gap: 4px; }
  fieldset { border: 1px solid var(--border); border-radius: var(--radius); padding: 6px 10px; display: grid; gap: 4px; margin: 0; }
  legend { color: var(--text-faint); font-size: var(--sc-t-meta); }
  .field, .type { display: grid; gap: 4px; }
  .field span, .type span { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .type { max-width: 380px; }
  .check { display: flex; gap: 6px; align-items: center; }
  .golden { display: grid; grid-template-columns: max-content 1fr; gap: 2px 10px; margin: 0 0 0 22px; font-size: var(--sc-t-meta); }
  .golden dt { color: var(--text-faint); }
  .golden dd { margin: 0; overflow-wrap: anywhere; }
  .sha { margin: 0; font-size: var(--sc-t-meta); overflow-wrap: anywhere; }
  .why { margin: 0; color: var(--warn-strong); }
  .refusal { margin: 0; padding: 8px 12px; border-left: 3px solid var(--warn-strong); background: var(--panel-raised); overflow-wrap: anywhere; }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); margin: 0; }
  .error { color: var(--error); margin: 0; overflow-wrap: anywhere; }
  .mono { font-family: var(--mono); }
  footer { display: flex; flex-wrap: wrap; gap: 8px; justify-content: flex-end; padding: 10px 16px; border-top: 1px solid var(--border); }
</style>
