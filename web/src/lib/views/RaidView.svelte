<script>
  // This node's shelves and RAID sets (#67, stormblock#252): each set's state, its rebuild and scrub, its
  // members by bay with a failed one marked, and the spares. What a change needs is the storage guard's
  // (#82): the actions come from the feed, withheld from anyone the review refuses.
  import { onMount } from 'svelte'
  import { get, call } from '../api.js'
  import { feed } from '../stores.svelte.js'
  import { HEALTH, stateWord, ordered, where, driveWords, needsHands, progress, memberActions, canScrub } from '../raid.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import StatusPill from '../components/StatusPill.svelte'

  const PROXY = '/api/plugins/sb/proxy/api/v1'

  let data = $state(null)
  let error = $state('')
  let said = $state('')
  let busy = $state(false)
  let mayAddSpare = $state(false)
  let replacing = $state('') // the member's component id
  let chosen = $state('')
  let spareFor = $state('') // the shelf's name
  let spareDrive = $state('')

  async function load() {
    try {
      data = await get('/api/plugins/sb/raid')
      error = data.error || ''
    } catch (e) {
      error = e.message
    }
  }

  onMount(() => {
    load()
    const t = setInterval(load, 3000)
    get(`/api/v1/console/guard?method=POST&path=${encodeURIComponent('/api/plugins/sb/proxy/api/v1/spares')}`)
      .then((g) => (mayAddSpare = !!g.guard?.allowed))
      .catch(() => {})
    return () => clearInterval(t)
  })

  async function act(what, fn) {
    busy = true
    said = ''
    try {
      await fn()
      said = what
      replacing = ''
      spareFor = ''
      await load()
    } catch (e) {
      if (e.message !== 'cancelled') said = `${what}: ${e.message}`
    }
    busy = false
  }

  const free = $derived(data?.free || [])
  const sets = $derived((data?.shelves || []).flatMap((s) => s.sets).concat(data?.arrays || []))
  const noSets = $derived(data && !sets.length && !(data.shelves || []).length)

  function failMember(m, a) {
    act(`${where(m)} failed`, () => call(a.method, a.path))
  }
  function replaceMember(m, a) {
    act(`Rebuilding ${where(m)} onto the chosen drive`, () => call(a.method, a.path, undefined, { drive_uuid: chosen }))
  }
  function scrub(s) {
    act(`Scrub of ${s.name || s.id} started`, () => call('POST', `${PROXY}/arrays/${s.id}/scrub`))
  }
  function addSpare(pool) {
    act(pool ? `Spare added to ${pool}` : 'Spare added', () => call('POST', `${PROXY}/spares`, undefined, { drive_uuid: spareDrive, pool }))
  }
</script>

{#snippet oneSet(s)}
  <article class="set" data-testid="set" data-state={s.status?.state}>
    <header>
      <span class="name">{s.name || s.id}</span>
      <StatusPill health={HEALTH[s.status?.state] || 'unknown'} label={stateWord(s.status?.state)} />
      <span class="dim">{s.level} · {s.capacity} · {s.volumes} volume{s.volumes === 1 ? '' : 's'}</span>
      {#if s.status?.failed}
        <span class="bad">{s.status.failed} failed, survives {s.status.tolerated}</span>
      {/if}
      <span class="grow"></span>
      {#if canScrub(s.status)}
        <button disabled={busy} onclick={() => scrub(s)} title="Reads every stripe and counts what disagrees. Changes nothing.">Scrub</button>
      {/if}
    </header>
    {#each progress(s.status) as p (p.kind)}
      <div class="progress" data-testid={p.kind}>
        <span class:bad={p.bad || p.error}>{p.text}{p.error ? `: ${p.error}` : ''}</span>
        <span class="bar"><span style={`width: ${Math.min(100, p.percent)}%`}></span></span>
        <span class="mono">{Math.round(p.percent)}%</span>
      </div>
    {/each}
    <div class="bays">
      {#each s.members as m (m.slot)}
        {@const a = memberActions(feed.components, m)}
        <div class="bay {m.state}" class:hands={needsHands(m)} data-testid="member" data-state={m.state}>
          <div class="top"><b>{where(m)}</b><span>{m.state}</span></div>
          <div class="mono drive" title={m.path}>{driveWords(m)}</div>
          {#if m.bay != null}<div class="dim">slot {m.slot}</div>{/if}
          {#if m.model}<div class="dim">{m.model}</div>{/if}
          {#if a.fail || a.replace}
            <div class="acts">
              {#if a.fail && m.state === 'active'}
                <button disabled={busy} onclick={() => failMember(m, a.fail)}>Fail</button>
              {/if}
              {#if a.replace}
                <button disabled={busy || !free.length} title={free.length ? '' : 'No free drive on this node'} onclick={() => { replacing = m.component; chosen = free[0]?.uuid || '' }}>Replace</button>
              {/if}
            </div>
            {#if replacing === m.component}
              <div class="ask">
                <select bind:value={chosen} aria-label="Replace with">
                  {#each free as d (d.uuid)}<option value={d.uuid}>{d.path} · {d.capacity}</option>{/each}
                </select>
                <button disabled={busy || !chosen} onclick={() => replaceMember(m, a.replace)}>Rebuild onto it</button>
                <button disabled={busy} onclick={() => (replacing = '')}>Cancel</button>
              </div>
            {/if}
          {/if}
        </div>
      {/each}
    </div>
  </article>
{/snippet}

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Storage' }, { label: 'RAID sets' }]} title="RAID sets" scope="this node's engine" count={data ? sets.length : null}>
    {#snippet status()}
      {#if data?.worst}<StatusPill health={HEALTH[data.worst] || 'unknown'} label={stateWord(data.worst)} />{/if}
    {/snippet}
  </PageHeader>

  {#if error}<p class="bad" data-testid="error">{error}</p>{/if}
  {#if said}<p class="said" data-testid="said">{said}</p>{/if}

  {#if !data && !error}
    <div class="sc-empty"><p>Asking the engine…</p></div>
  {:else if noSets}
    <EmptyState icon="storage" title="No RAID sets on this node" hint="A shelf is laid out through the engine's API, POST /api/v1/shelves. Nothing here makes one." />
  {:else if data}
    {#each data.shelves as sh (sh.name)}
      <section class="shelf" aria-label={`Shelf ${sh.name}`}>
        <h2>
          Shelf {sh.name}
          <StatusPill health={HEALTH[sh.state] || 'unknown'} label={stateWord(sh.state)} />
          <span class="dim">{sh.usable} usable · {sh.spares.length} spare{sh.spares.length === 1 ? '' : 's'}</span>
        </h2>
        {#each ordered(sh.sets) as s (s.id)}{@render oneSet(s)}{/each}
        <div class="spares" data-testid="spares">
          <b>Spares</b>
          {#each sh.spares as sp (sp.drive)}<span class="spare mono">{driveWords(sp)}</span>{/each}
          {#if !sh.spares.length}<span class="warnword">none: a failed member waits for a person</span>{/if}
          {#if mayAddSpare && free.length}
            {#if spareFor === sh.name}
              <select bind:value={spareDrive} aria-label="Add as a spare">
                {#each free as d (d.uuid)}<option value={d.uuid}>{d.path} · {d.capacity}</option>{/each}
              </select>
              <button disabled={busy || !spareDrive} onclick={() => addSpare(sh.name)}>Add it</button>
              <button disabled={busy} onclick={() => (spareFor = '')}>Cancel</button>
            {:else}
              <button disabled={busy} onclick={() => { spareFor = sh.name; spareDrive = free[0].uuid }}>Add a spare</button>
            {/if}
          {/if}
        </div>
      </section>
    {/each}
    {#if data.arrays.length}
      <section class="shelf" aria-label="Arrays on no shelf">
        <h2>On no shelf</h2>
        {#each ordered(data.arrays) as s (s.id)}{@render oneSet(s)}{/each}
      </section>
    {/if}
    {#if data.globalSpares.length}
      <p class="dim">Spares for any shelf: {data.globalSpares.map(driveWords).join(', ')}</p>
    {/if}
  {/if}
</div>

<style>
  .shelf { margin-bottom: 18px; }
  h2 { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; font-size: var(--sc-t-body); font-weight: 600; margin: 0 0 8px; }
  .set, .spares {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px var(--sc-row-px);
    margin-bottom: 8px;
  }
  .set[data-state='failed'] { border-color: var(--error); }
  .set[data-state='degraded'], .set[data-state='rebuilding'] { border-color: var(--warn-strong); }
  header { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 12px; }
  .name { font-weight: 600; }
  .grow { flex: 1; }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); font-weight: 400; }
  .bad { color: var(--error); font-size: var(--sc-t-meta); overflow-wrap: anywhere; }
  .warnword { color: var(--warn-strong); font-size: var(--sc-t-meta); }
  .said { font-size: var(--sc-t-meta); color: var(--text-dim); overflow-wrap: anywhere; }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .progress { display: grid; grid-template-columns: minmax(0, 1fr) 160px 40px; align-items: center; gap: 10px; margin-top: 8px; font-size: var(--sc-t-meta); }
  .bar { display: block; height: 6px; background: var(--border); border-radius: 3px; overflow: hidden; }
  .bar span { display: block; height: 100%; background: var(--accent); }
  .bays { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 6px; margin-top: 10px; }
  .bay { border: 1px solid var(--border); border-radius: var(--radius); padding: 6px 8px; font-size: var(--sc-t-meta); min-width: 0; }
  .bay .top { display: flex; justify-content: space-between; gap: 6px; }
  .bay .top span { color: var(--text-dim); }
  .bay .drive { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .bay.rebuilding { border-color: var(--warn-strong); }
  .bay.rebuilding .top span { color: var(--warn-strong); }
  .bay.hands { border: 2px solid var(--error); background: color-mix(in srgb, var(--error) 12%, transparent); }
  .bay.hands .top span { color: var(--error); font-weight: 600; }
  .acts, .ask { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 6px; }
  .ask select { max-width: 100%; min-width: 0; }
  .spares { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; font-size: var(--sc-t-meta); }
  .spare { border: 1px solid var(--border); border-radius: var(--radius); padding: 1px 6px; max-width: 100%; overflow: hidden; text-overflow: ellipsis; }
</style>
