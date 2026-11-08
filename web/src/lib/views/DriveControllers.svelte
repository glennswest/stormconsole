<script>
  // RAID controllers (#325, stormdrive#132): each node's PERCs — model,
  // firmware, mode, how the mode is known, what is pending — and each disk as
  // the controller sees it. A controller in RAID mode hides its disks from
  // Linux, so HBA mode is required: Switch to HBA asks for it (a reboot of the
  // node applies it). A disk the controller has failed is "Failed by the
  // controller", not missing, and Set good (or Set JBOD) brings it back.
  // Every verb is a drive operation: a storage-admin's, as the person, with
  // the word typed (#82). The model is controllers.js.
  import { get } from '../api.js'
  import { feed } from '../stores.svelte.js'
  import { build, formatBytes } from '../drivemap.js'
  import { byNode, counts, hbaWhy, modeText, personalityPath, stateBody, statePath, stateText, stateWhy, isFailed } from '../controllers.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import ControllerBanner from '../components/ControllerBanner.svelte'
  import ControllerDialog from '../components/ControllerDialog.svelte'

  let data = $state(null)
  let error = $state('')
  let usage = $state(null)
  let guards = $state({})
  let ask = $state(null)
  let note = $state(null)

  async function refresh() {
    try {
      data = await get('/api/plugins/drive/controllers')
      error = ''
    } catch (e) {
      error = e.message
    }
    get('/api/plugins/drive/usage').then((u) => (usage = u)).catch(() => {})
  }
  $effect(() => {
    refresh()
    const t = setInterval(refresh, 5000)
    return () => clearInterval(t)
  })
  const list = $derived(data?.controllers || [])
  const groups = $derived(byNode(list))
  const total = $derived(counts(list))
  const quiet = $derived((data?.nodes || []).filter((n) => !n.ok || n.predates))
  const rows = $derived(new Map(build(feed.components, { usage }).records.map((r) => [r.id, r])))

  // Whether this person may, asked once per controller (the answer is per verb, not per object).
  // `asked` is not state, so the effect never re-runs itself.
  const asked = new Set()
  $effect(() => {
    for (const c of list) {
      const p = personalityPath(c)
      if (asked.has(p)) continue
      asked.add(p)
      get(`/api/v1/console/guard?method=POST&path=${encodeURIComponent(p)}`)
        .then((g) => (guards = { ...guards, [p]: g.guard }))
        .catch((e) => (guards = { ...guards, [p]: { allowed: false, reason: e.message } }))
    }
  })
  const guardOf = (c) => guards[personalityPath(c)] || null

  function switchHba(c) {
    ask = {
      title: `Switch ${c.node}'s ${c.model || 'controller'} to HBA`,
      verb: 'Switch to HBA',
      source: `drives/${c.node}`,
      path: personalityPath(c),
      body: { mode: 'HBA' },
      lines: [
        `${c.key} on ${c.node} is in ${modeText(c.mode)} mode (${c.mode_basis || 'how it is known was not said'}).`,
        'HBA mode passes every disk to Linux as it is: no virtual disks, no controller states.',
        `perccli sets the personality now; it takes effect when ${c.node} is rebooted, which this does not do.`,
        'No data is touched. It is refused while the controller has virtual disks.',
      ],
      done: (r) => `${c.key} on ${c.node}: HBA asked for${r?.reboot_required ? `; reboot ${c.node} to apply it` : ''}`,
    }
  }
  function setState(c, d, target) {
    const name = d.name || `device ${d.device_id}`
    ask = {
      title: `${target === 'good' ? 'Set good' : 'Set JBOD'}: ${name} on ${c.node}`,
      verb: target === 'good' ? 'Set good' : 'Set JBOD',
      source: `drives/${c.node}`,
      path: statePath(c, d),
      body: stateBody(target),
      lines: [
        `${c.key} slot ${d.slot ?? '?'}: ${stateText(d.state)}.`,
        target === 'good'
          ? 'The controller marks it unconfigured good (perccli’s set good force). In RAID mode Linux still does not see it until it is JBOD.'
          : 'The controller marks it good, then JBOD (non-RAID): Linux sees the drive as it is.',
        'The controller’s own refusal, and stormdrive’s (the system drive, a fleet or in-use drive, a member of a virtual disk), is said here.',
      ],
      done: (r) => `${name} on ${c.node}: the controller reads ${r?.state || target}`,
    }
  }
</script>

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Hardware' }, { label: 'Drives', href: '#/drives' }, { label: 'Controllers' }]} title="Controllers" count={data ? total.controllers : null} />

  <ControllerBanner controllers={list} link={false} />

  {#if note}
    <p class="note" class:err={!note.ok} role="status" data-testid="controller-note">{note.text} <button class="link" onclick={() => (note = null)}>dismiss</button></p>
  {/if}

  {#if error && !data}
    <EmptyState icon="storage" title="Controllers could not be read" hint={error} />
  {:else if !data}
    <div class="sc-empty"><p>Reading each node’s controllers…</p></div>
  {:else if !list.length}
    <EmptyState
      icon="storage"
      title="No RAID controllers"
      hint="No node’s stormdrive reports a MegaRAID (PERC) controller. Drives behind a plain HBA need nothing here."
    />
  {:else}
    <div class="band">
      <span><strong>{total.controllers}</strong> controller{total.controllers === 1 ? '' : 's'}</span>
      <span class:warn={total.raid}><strong>{total.raid}</strong> in RAID mode</span>
      {#if total.pending}<span><strong>{total.pending}</strong> waiting for a reboot</span>{/if}
      <span><strong>{total.disks}</strong> disks</span>
      <span class:err={total.failed}><strong>{total.failed}</strong> failed by the controller</span>
    </div>
    {#each groups as g (g.node)}
      <h2 class="node">{g.node}</h2>
      {#each g.controllers as c (c.key)}
        {@const why = hbaWhy(c, guardOf(c))}
        <section class="ctl" data-testid="controller" data-key={c.key} data-node={c.node}>
          <header>
            <strong>{c.model || 'RAID controller'}</strong>
            <span class="mode" class:raid={c.mode === 'raid'} data-testid="controller-mode">{modeText(c.mode)} mode</span>
            {#if c.pending_mode}<span class="pending" data-testid="controller-pending">{modeText(c.pending_mode)} after reboot</span>{/if}
            <span class="tools">
              <button
                class="primary"
                disabled={!!why}
                title={why}
                onclick={() => switchHba(c)}
                data-testid="switch-hba">Switch to HBA…</button>
            </span>
          </header>
          {#if why && c.mode !== 'hba'}<p class="why" data-testid="switch-hba-why">Switch to HBA: {why}.</p>{/if}
          <dl class="facts">
            <dt>Address</dt><dd>{c.key}{c.scsi_host ? ` · ${c.scsi_host}` : ''}</dd>
            <dt>Firmware</dt><dd>{c.firmware || '—'}</dd>
            <dt>Serial</dt><dd>{c.serial || '—'}</dd>
            <dt>Mode</dt><dd>{modeText(c.mode)} <span class="dim">({c.mode_basis || 'basis not said'})</span></dd>
            <dt>Virtual disks</dt><dd>{c.virtual_disks ?? '—'}</dd>
            <dt>JBOD</dt><dd>{c.jbod_enabled === true ? 'enabled' : c.jbod_enabled === false ? 'disabled' : '—'}</dd>
            <dt>perccli</dt><dd>{c.perccli?.ok ? c.perccli.path : `not available: ${c.perccli?.why || 'not said'}`}</dd>
            {#if c.error}<dt>Status</dt><dd class="err">not read: {c.error}</dd>{:else}<dt>Status</dt><dd>read</dd>{/if}
          </dl>
          {#if (c.findings || []).length}
            <ul class="findings">
              {#each c.findings as f}<li class={f.severity} data-code={f.code}>{f.message}</li>{/each}
            </ul>
          {/if}
          <div class="scroll">
            <table class="disks">
              <thead><tr><th>Slot</th><th>Drive</th><th>Controller state</th><th>Model</th><th class="n">Offered</th><th class="n">Errors</th><th></th></tr></thead>
              <tbody>
                {#each c.drives || [] as d (d.device_id)}
                  {@const row = d.component ? rows.get(d.component) : null}
                  {@const goodWhy = stateWhy(d, 'good', row, guardOf(c))}
                  {@const jbodWhy = stateWhy(d, 'jbod', row, guardOf(c))}
                  <tr class:failed={isFailed(d.state)} data-testid="controller-disk" data-device={d.device_id} data-name={d.name || ''}>
                    <td>{d.enclosure ?? '—'}:{d.slot ?? '?'}</td>
                    <td>{d.name || 'no drive record'}{row?.serial ? ` · ${row.serial}` : ''}</td>
                    <td class="state" data-testid="controller-state">{stateText(d.state)}</td>
                    <td>{[d.vendor, d.model].filter(Boolean).join(' ') || '—'}</td>
                    <td class="n">{d.size_bytes ? formatBytes(d.size_bytes) : '0'}</td>
                    <td class="n" title="media / other / predictive failures">{d.media_errors ?? 0} / {d.other_errors ?? 0} / {d.predictive_failures ?? 0}</td>
                    <td class="acts">
                      {#if isFailed(d.state) || d.state === 'unconfigured_good'}
                        <button disabled={!!goodWhy} title={goodWhy} onclick={() => setState(c, d, 'good')} data-testid="set-good">Set good…</button>
                        <button disabled={!!jbodWhy} title={jbodWhy} onclick={() => setState(c, d, 'jbod')} data-testid="set-jbod">Set JBOD…</button>
                        {#if goodWhy && jbodWhy}<span class="why small" data-testid="set-good-why">{goodWhy}</span>{/if}
                      {/if}
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </section>
      {/each}
    {/each}
  {/if}
  {#if quiet.length}
    <p class="dim" data-testid="controllers-quiet">
      {#each quiet as n, i}{i ? '; ' : ''}{n.node}: {n.predates ? n.note : `not read (${n.error})`}{/each}
    </p>
  {/if}
</div>

{#if ask}
  <ControllerDialog
    {ask}
    onclose={() => (ask = null)}
    ondone={(said) => {
      ask = null
      note = { ok: true, text: said }
      refresh()
    }}
  />
{/if}

<style>
  .band { display: flex; flex-wrap: wrap; gap: 6px 18px; margin: 0 0 12px; font-size: var(--sc-t-body); color: var(--text-dim); }
  .band strong { color: var(--text); }
  .band .warn strong { color: var(--warn-strong); }
  .band .err strong { color: var(--error); }
  h2.node { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 16px 0 6px; }
  .ctl { border: 1px solid var(--border); border-radius: var(--radius); background: var(--panel); padding: 10px 12px; margin: 0 0 12px; }
  .ctl header { display: flex; flex-wrap: wrap; gap: 6px 10px; align-items: center; }
  .tools { margin-left: auto; }
  .mode, .pending { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; padding: 1px 6px; border-radius: 999px; border: 1px solid var(--border-strong); color: var(--text-dim); }
  .mode.raid { border-color: var(--warn-strong); color: var(--warn-strong); }
  .pending { border-color: var(--accent); color: var(--accent); }
  .why { margin: 6px 0 0; font-size: var(--sc-t-meta); color: var(--warn-strong); overflow-wrap: anywhere; }
  .why.small { display: block; margin: 2px 0 0; }
  .facts { display: grid; grid-template-columns: max-content 1fr; gap: 2px 12px; margin: 8px 0; font-size: var(--sc-t-meta); }
  .facts dt { color: var(--text-faint); }
  .facts dd { margin: 0; overflow-wrap: anywhere; }
  .findings { margin: 4px 0 8px; padding-left: 18px; font-size: var(--sc-t-meta); }
  .findings .warning { color: var(--warn-strong); }
  .scroll { overflow-x: auto; }
  .disks { width: 100%; border-collapse: collapse; font-size: var(--sc-t-meta); }
  .disks th { text-align: left; color: var(--text-faint); font-weight: 500; padding: 4px 6px; border-bottom: 1px solid var(--border); }
  .disks td { padding: 5px 6px; border-top: 1px solid var(--sc-hairline); vertical-align: top; }
  .disks .n { text-align: right; font-variant-numeric: tabular-nums; }
  .disks tr.failed .state { color: var(--error); font-weight: 600; }
  .acts { white-space: nowrap; }
  .acts button { font-size: var(--sc-t-meta); padding: 2px 8px; }
  .acts .why { white-space: normal; max-width: 260px; }
  .note { margin: 0 0 10px; font-size: var(--sc-t-body); }
  .note.err { color: var(--error); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .err { color: var(--error); }
  .link { background: none; border: none; color: var(--accent); padding: 0; cursor: pointer; }
</style>
