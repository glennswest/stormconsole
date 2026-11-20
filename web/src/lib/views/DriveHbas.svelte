<script>
  // SAS cards (#376, stormdrive#166): each node's HostBusAdapter objects, read
  // as you from the apiserver. Identity (board, chip, SAS address, the
  // firmware, BIOS and NVDATA versions, IOC Facts), each phy (its rate, what
  // the card allows, the drive on it and that drive's link level), the
  // firmware regions read back with their sha256, the last flash, and every
  // CardOperation: its phase, step and bytes, before → after, and a refusal in
  // the node's words. Flash… creates one, as you (storage-admin), with the
  // card's name typed. The model is hba.js.
  import { get } from '../api.js'
  import { route } from '../router.svelte.js'
  import { formatBytes } from '../drivemap.js'
  import { busy, byNode, flashWhy, limitText, phaseText, phaseTone, progress, rateText, slow, versions } from '../hba.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import FlashDialog from '../components/FlashDialog.svelte'

  let data = $state(null)
  let error = $state('')
  let links = $state(null)
  let store = $state(null)
  let storeError = $state('')
  let guards = $state({})
  let flashing = $state(null)
  let note = $state(null)

  async function refresh() {
    try {
      data = await get('/api/plugins/k8s/hbas')
      error = ''
    } catch (e) {
      error = e.message
    }
    get('/api/plugins/drive/links').then((l) => (links = l)).catch(() => {})
  }
  // Every 3 s while an operation is going, else every 10 s.
  $effect(() => {
    let stop = false
    let t = null
    const tick = async () => {
      await refresh()
      if (!stop) t = setTimeout(tick, busy(data?.items) ? 3000 : 10000)
    }
    tick()
    return () => {
      stop = true
      clearTimeout(t)
    }
  })

  const only = $derived(route.current.query.get('card'))
  const all = $derived(data?.items || [])
  const list = $derived(only ? all.filter((c) => c.name === only) : all)
  const groups = $derived(byNode(list))
  const linkOf = $derived(new Map((links?.drives || []).map((r) => [r.id, r])))

  // Whether this person may flash, asked once per card.
  const asked = new Set()
  $effect(() => {
    for (const c of all) {
      const p = `/api/plugins/k8s/hbas/${encodeURIComponent(c.name)}/flash`
      if (asked.has(p)) continue
      asked.add(p)
      get(`/api/v1/console/guard?method=POST&path=${encodeURIComponent(p)}`)
        .then((g) => (guards = { ...guards, [c.name]: g.guard }))
        .catch((e) => (guards = { ...guards, [c.name]: { allowed: false, reason: e.message } }))
    }
  })

  async function openFlash(c) {
    flashing = c
    store = null
    storeError = ''
    try {
      const fw = await get('/api/plugins/drive/firmware')
      store = fw.images || []
      const n = (fw.nodes || []).find((x) => x.node === c.status?.node)
      if (n && n.store !== true) storeError = n.error || n.store || 'not answered'
      else if (!n) storeError = `the drive plugin reads no stormdrive called ${c.status?.node}`
    } catch (e) {
      store = []
      storeError = e.message
    }
  }
  const at = (t) => (t ? String(t).replace('T', ' ').replace('Z', '') : '—')
</script>

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Hardware' }, { label: 'Drives', href: '#/drives' }, { label: 'SAS cards', href: only ? '#/drives/hbas' : undefined }, ...(only ? [{ label: only }] : [])]} title={only || 'SAS cards'} count={data ? list.length : null} />

  {#if note}
    <p class="note" class:err={!note.ok} role="status" data-testid="hba-note">{note.text} <button class="link" onclick={() => (note = null)}>dismiss</button></p>
  {/if}

  {#if error && !data}
    <EmptyState icon="storage" title="SAS cards could not be read" hint={error} />
  {:else if !data}
    <div class="sc-empty"><p>Reading the HostBusAdapter objects…</p></div>
  {:else if !list.length}
    <EmptyState
      icon="storage"
      title={only ? `No SAS card ${only}` : 'No SAS cards'}
      hint={only ? 'It is not a HostBusAdapter you may read.' : 'No node’s stormdrive has written a HostBusAdapter (one per mpt2sas/mpt3sas card, stormdrive 0.56.0) that you may read.'}
    />
  {:else}
    {#each groups as g (g.node)}
      <h2 class="node">{g.node}</h2>
      {#each g.cards as c (c.name)}
        {@const st = c.status || {}}
        {@const why = flashWhy(c, guards[c.name] || null)}
        <section class="card" data-testid="hba" data-name={c.name} data-node={st.node}>
          <header>
            <strong>{st.board || st.adapter || 'SAS card'}</strong>
            <span class="dim">{c.name}</span>
            {#if st.present === false}<span class="pill err">not seen</span>{/if}
            <span class="tools">
              {#if only}<a href="#/cr/hostbusadapters.storage.storm.io/_/{encodeURIComponent(c.name)}">Object</a>{:else}<a href="#/drives/hbas?card={encodeURIComponent(c.name)}" data-testid="hba-open">Open</a>{/if}
              <button class="primary" disabled={!!why} title={why} onclick={() => openFlash(c)} data-testid="hba-flash">Flash…</button>
            </span>
          </header>
          {#if why && guards[c.name]}<p class="why" data-testid="hba-flash-why">Flash: {why}.</p>{/if}
          <dl class="facts" data-testid="hba-identity">
            <dt>PCI</dt><dd>{st.pci || '—'}{st.scsiHost ? ` · ${st.scsiHost}` : ''}{st.pciDevice ? ` · device ${st.pciDevice}` : ''}{st.subsystem ? ` · subsystem ${st.subsystem}` : ''}</dd>
            <dt>Chip</dt><dd>{st.chip || '—'}{st.chipRevision ? ` rev ${st.chipRevision}` : ''}{st.adapter ? ` · ${st.adapter}` : ''}</dd>
            <dt>Board</dt><dd>{st.board || '—'}{st.boardAssembly ? ` · assembly ${st.boardAssembly}` : ''}{st.boardTracer ? ` · tracer ${st.boardTracer}` : ''}</dd>
            <dt>SAS address</dt><dd class="mono">{st.sasAddress || '—'}</dd>
            <dt>Firmware</dt><dd data-testid="hba-fw">{st.firmwareVersion || '—'}</dd>
            <dt>BIOS</dt><dd>{st.biosVersion || '—'}</dd>
            <dt>NVDATA</dt><dd>{st.nvdataVersion || '—'}{st.nvdataDefaultVersion && st.nvdataDefaultVersion !== st.nvdataVersion ? ` (default ${st.nvdataDefaultVersion})` : ''}</dd>
            <dt>Driver</dt><dd>{st.driver || '—'}{st.controlDevice ? ` · ${st.controlDevice}` : ''}</dd>
            {#if st.facts}
              <dt>IOC Facts</dt><dd>MPI {st.facts.mpiVersion} · product {st.facts.productId} · {st.facts.ports} port{st.facts.ports === 1 ? '' : 's'} · {st.facts.maxTargets} targets · {st.facts.maxEnclosures} enclosures · credit {st.facts.requestCredit}</dd>
            {/if}
            <dt>Drives</dt><dd>{st.drives ?? 0} on {st.linkedPhys ?? 0} linked phy{st.linkedPhys === 1 ? '' : 's'}</dd>
          </dl>
          {#if (st.problems || []).length || (st.errors || []).length}
            <ul class="findings" data-testid="hba-problems">
              {#each st.problems || [] as p}<li class="warning">{p}</li>{/each}
              {#each st.errors || [] as e}<li class="dim">not read: {e}</li>{/each}
            </ul>
          {/if}

          <h3>Phys</h3>
          <div class="scroll">
            <table data-testid="hba-phys">
              <thead><tr><th class="n">Phy</th><th class="n">Port</th><th>Rate</th><th>Card allows</th><th>Attached</th><th>Link</th></tr></thead>
              <tbody>
                {#each st.phys || [] as p (p.phy)}
                  {@const d = p.attached}
                  {@const l = d?.drive ? linkOf.get(d.drive) : null}
                  <tr class:off={p.disabled || !p.rateMbps} data-testid="hba-phy" data-phy={p.phy}>
                    <td class="n">{p.phy}</td>
                    <td class="n">{p.port ?? '—'}</td>
                    <td class:warn={slow(p)} title={slow(p) ? 'below what the card allows: the drive, the cable or a policy holds it' : ''}>{rateText(p)}</td>
                    <td>{limitText(p)}</td>
                    <td>
                      {#if d}
                        {#if d.drive && l}<a href="#/drives?drive={encodeURIComponent(l.component)}" data-testid="hba-phy-drive">{d.name || d.drive}</a>{:else}{d.name || d.drive || d.kind}{/if}
                        <span class="dim">{d.kind} · <span class="mono">{d.sasAddress}</span></span>
                      {:else}—{/if}
                    </td>
                    <td>{#if l?.link?.level}<span class="lvl {l.link.level}" data-testid="hba-phy-level">{l.link.level}</span>{:else}<span class="dim">—</span>{/if}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>

          {#if (st.enclosures || []).length}
            <p class="dim">Enclosures: {#each st.enclosures as e, i}{i ? '; ' : ''}<span class="mono">{e.logicalId}</span> ({e.slots} slots{e.shelf ? `, shelf ${e.shelf}` : ''}){/each}</p>
          {/if}

          <h3>Firmware on the card</h3>
          <div class="scroll">
            <table data-testid="hba-images">
              <thead><tr><th>Region</th><th>Version</th><th class="n">Size</th><th>sha256</th></tr></thead>
              <tbody>
                {#each st.firmware?.images || [] as i}
                  <tr data-testid="hba-image" data-region={i.region}>
                    <td>{i.region}{i.part ? ` · ${i.part}` : ''}</td>
                    <td>{i.version || '—'}{i.name ? ` · ${i.name}` : ''}{i.productId ? ` · product ${i.productId}` : ''}</td>
                    <td class="n">{formatBytes(i.size)}</td>
                    <td class="mono sha">{i.sha256}</td>
                  </tr>
                {:else}
                  <tr><td colspan="4" class="dim">Not read back yet (once per stormdrive run, and after each flash).</td></tr>
                {/each}
              </tbody>
            </table>
          </div>
          {#if st.firmware?.readAt}<p class="dim">Read back {at(st.firmware.readAt)}.</p>{/if}
          {#if st.lastFlash}
            <p class="dim" data-testid="hba-last-flash">Last flash: {st.lastFlash.region} with {st.lastFlash.image} ({st.lastFlash.before || '?'} → {st.lastFlash.after || '?'}), {at(st.lastFlash.finished)}: {st.lastFlash.result}</p>
          {/if}

          <h3>Operations</h3>
          {#if data.operationsError}
            <p class="dim" data-testid="hba-ops-error">Not read: {data.operationsError}.</p>
          {:else if !(c.operations || []).length}
            <p class="dim">None.</p>
          {:else}
            <div class="scroll">
              <table data-testid="hba-ops">
                <thead><tr><th>Operation</th><th>Phase</th><th>Region · image</th><th>Versions</th><th>By</th><th>When</th></tr></thead>
                <tbody>
                  {#each c.operations as op (op.name)}
                    {@const pr = progress(op)}
                    <tr data-testid="hba-op" data-name={op.name} data-phase={op.phase}>
                      <td class="mono">{op.name}{op.dryRun ? ' (dry run)' : ''}</td>
                      <td>
                        <span class="phase {phaseTone(op)}">{op.phase || 'new'}</span> <span class="dim">{phaseText(op)}</span>
                        {#if pr}<div class="bar" title={pr.text}><span style="width: {pr.fraction === null ? 100 : pr.fraction * 100}%" class:indef={pr.fraction === null}></span></div><span class="dim">{pr.text}</span>{/if}
                        {#if op.message}<div class="msg" class:err={phaseTone(op) === 'error'} data-testid="hba-op-message">{op.message}</div>{/if}
                      </td>
                      <td>{op.region} · {op.image?.name}<div class="dim mono sha">{op.image?.sha256}</div><div class="dim">from {op.image?.source}</div></td>
                      <td data-testid="hba-op-versions">{versions(op)}</td>
                      <td>{op.requester || '—'}</td>
                      <td>{at(op.started || op.created)}{op.finished ? ` → ${at(op.finished)}` : ''}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        </section>
      {/each}
    {/each}
    {#if !only && (data.orphans || []).length}
      <h2 class="node">Operations on no card you can see</h2>
      <ul class="findings">
        {#each data.orphans as op (op.name)}<li data-testid="hba-orphan">{op.name}: {op.card} on {op.node}, {op.phase || 'new'}{op.message ? ` — ${op.message}` : ''}</li>{/each}
      </ul>
    {/if}
  {/if}
</div>

{#if flashing && store}
  <FlashDialog
    card={flashing}
    images={store}
    {storeError}
    onclose={() => (flashing = null)}
    ondone={(said) => {
      flashing = null
      note = { ok: true, text: said }
      refresh()
    }}
  />
{/if}

<style>
  h2.node { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 16px 0 6px; }
  h3 { font-size: var(--sc-t-meta); color: var(--text-faint); font-weight: 600; margin: 10px 0 4px; }
  .card { border: 1px solid var(--border); border-radius: var(--radius); background: var(--panel); padding: 10px 12px; margin: 0 0 12px; }
  .card header { display: flex; flex-wrap: wrap; gap: 6px 10px; align-items: center; }
  .tools { margin-left: auto; display: flex; gap: 10px; align-items: center; }
  .pill { font-size: var(--sc-t-eyebrow); text-transform: uppercase; padding: 1px 6px; border-radius: 999px; border: 1px solid var(--error); color: var(--error); }
  .why { margin: 6px 0 0; font-size: var(--sc-t-meta); color: var(--warn-strong); overflow-wrap: anywhere; }
  .facts { display: grid; grid-template-columns: max-content 1fr; gap: 2px 12px; margin: 8px 0; font-size: var(--sc-t-meta); }
  .facts dt { color: var(--text-faint); }
  .facts dd { margin: 0; overflow-wrap: anywhere; }
  .findings { margin: 4px 0 8px; padding-left: 18px; font-size: var(--sc-t-meta); }
  .findings .warning { color: var(--warn-strong); }
  .scroll { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-meta); }
  th { text-align: left; color: var(--text-faint); font-weight: 500; padding: 4px 6px; border-bottom: 1px solid var(--border); }
  td { padding: 5px 6px; border-top: 1px solid var(--sc-hairline); vertical-align: top; }
  .n { text-align: right; font-variant-numeric: tabular-nums; }
  tr.off td { color: var(--text-dim); }
  td.warn { color: var(--warn-strong); }
  .sha { overflow-wrap: anywhere; word-break: break-all; }
  .lvl.ok, .phase.ok { color: var(--ok); }
  .lvl.degraded, .lvl.settling, .phase.warn { color: var(--warn-strong); }
  .lvl.failing, .phase.error { color: var(--error); }
  .phase.accent { color: var(--accent); }
  .phase { font-weight: 600; }
  .msg { margin-top: 2px; overflow-wrap: anywhere; }
  .msg.err { color: var(--error); }
  .bar { height: 6px; background: var(--panel-raised); border-radius: 3px; overflow: hidden; margin: 4px 0 2px; max-width: 220px; }
  .bar span { display: block; height: 100%; background: var(--accent); }
  .bar span.indef { opacity: 0.5; }
  .note { margin: 0 0 10px; font-size: var(--sc-t-body); }
  .note.err, .err { color: var(--error); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .mono { font-family: var(--mono); }
  .link { background: none; border: none; color: var(--accent); padding: 0; cursor: pointer; }
</style>
