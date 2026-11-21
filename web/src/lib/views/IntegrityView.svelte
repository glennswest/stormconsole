<script>
  // Every RAID set's integrity (#368, stormraid#80): what the set found and
  // rewrote from parity, for life and for the last day, its scrub (running,
  // how often, passes done, the next, the last pass), `BitRot` with the
  // members it names, and each member drive's counters, linked to the drive.
  // From the drive plugin's GET /integrity, which reads every RaidSet once a
  // minute and keeps a day of samples. One set is `?set=`. The model is
  // integrity.js.
  import { get } from '../api.js'
  import { route } from '../router.svelte.js'
  import { counterRows, whereLine, dayWords, scrubLine, passLine, setLevel, levelTone, rotting, at, setTrend, TREND_WINDOW } from '../integrity.js'
  import EntityCharts from '../components/EntityCharts.svelte'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'

  let data = $state(null)
  let error = $state('')
  async function refresh() {
    try {
      data = await get('/api/plugins/drive/integrity')
      error = ''
    } catch (e) {
      error = e.message
    }
  }
  $effect(() => {
    refresh()
    const t = setInterval(refresh, 15000)
    return () => clearInterval(t)
  })

  const only = $derived(route.current.query.get('set'))
  const all = $derived(data?.sets || [])
  const list = $derived(only ? all.filter((s) => s.name === only) : all)
  const lvl = $derived(new Map(all.map((s) => [s.name, setLevel(s)])))
  const words = { lost: 'rows past parity', bitrot: 'BitRot', ok: 'clean', unknown: 'no counters' }
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Hardware' }, { label: 'Drives', href: '#/drives' }, { label: 'Integrity', href: only ? '#/drives/integrity' : undefined }, ...(only ? [{ label: only }] : [])]}
    title={only || 'Integrity'}
    count={data ? list.length : null}
  />

  {#if error && !data}
    <EmptyState icon="storage" title="Integrity could not be read" hint={error} />
  {:else if !data}
    <div class="sc-empty"><p>Reading the RAID sets…</p></div>
  {:else if !data.kube}
    <EmptyState icon="storage" title="No apiserver" hint="RAID sets are RaidSet objects on the apiserver, and this console has none configured ([kubernetes])." />
  {:else if !data.read}
    <div class="sc-empty"><p>The RAID sets are read once a minute; the first read has not finished.</p></div>
  {:else if data.absent}
    <EmptyState icon="storage" title="No RAID sets" hint={data.error} />
  {:else if !list.length}
    <EmptyState
      icon="storage"
      title={only ? `No RAID set ${only}` : 'No RAID sets'}
      hint={data.error || (only ? 'The apiserver has no RaidSet of that name.' : 'The apiserver has no RaidSet objects.')}
    />
  {:else}
    {#if data.error}<p class="note err" role="status" data-testid="integrity-error">{data.error}. What is shown is the last read.</p>{/if}
    {#each list as s (s.name)}
      {@const level = lvl.get(s.name)}
      {@const integ = s.integrity}
      <section class="set" data-testid="integrity-set" data-name={s.name}>
        <header>
          <strong>{s.name}</strong>
          <span class="dim">{s.node || 'no node'}{s.parity !== null && s.parity !== undefined ? ` · parity ${s.parity}` : ''}{s.pi && s.pi !== 'off' ? ` · PI ${s.pi}` : ''}{s.phase ? ` · ${s.phase}` : ''}{s.device ? ` · ${s.device}` : ''}</span>
          <span class="pill {levelTone(level)}" data-testid="integrity-level">{words[level]}</span>
          <span class="tools">
            {#if only}<a href="#/cr/raidsets.storage.storm.io/_/{encodeURIComponent(s.name)}">Object</a>{:else}<a href="#/drives/integrity?set={encodeURIComponent(s.name)}" data-testid="integrity-open">Open</a>{/if}
          </span>
        </header>
        {#if rotting(s)}
          <p class="warn" data-testid="integrity-bitrot">BitRot{s.bitrot.message ? `: ${s.bitrot.message}` : ''}</p>
        {/if}
        {#if !integ}
          <p class="dim">This set reports no integrity counters: its node’s stormraid predates v0.27.0.</p>
        {:else}
          <p class="dim small" data-testid="integrity-scrub">Scrub: {scrubLine(integ)}</p>
          {#if integ.lastScrubPass}<p class="dim small" data-testid="integrity-pass">Last pass: {passLine(integ.lastScrubPass)}</p>{/if}
          {#if s.lastScrub}<p class="dim small">Last scrub asked for ({s.lastScrub.request}): {s.lastScrub.state}{s.lastScrub.finishedAt ? `, ${at(s.lastScrub.finishedAt)}` : ''}{s.lastScrub.message ? ` — ${s.lastScrub.message}` : ''}</p>{/if}
          <table class="counters" data-testid="integrity-totals">
            <thead><tr><th>The set</th><th class="n">Lifetime</th><th class="n">{dayWords(integ.day)}</th><th>Last rose</th></tr></thead>
            <tbody>
              {#each counterRows(integ.totals, integ.day, integ.last) as r (r.key)}
                <tr data-key={r.key}>
                  <td title={r.title}>{r.word}</td>
                  <td class="n mono">{r.life}</td>
                  <td class="n mono {r.tone}">{r.day === null ? '—' : r.day}</td>
                  <td class="dim small">{r.last ? at(r.last) : '—'}</td>
                </tr>
              {/each}
            </tbody>
          </table>
          {#if integ.totals?.mismatches}<p class="dim small">Mismatches found: {whereLine(integ.totals)}.</p>{/if}
        {/if}
        {#if (s.members || []).length}
          <table class="counters members" data-testid="integrity-members">
            <thead>
              <tr><th>Slot</th><th>Drive</th><th>Role</th><th class="n">Mismatches</th><th class="n">Rewritten</th><th class="n">PI</th><th class="n">Read</th><th class="n">Write</th></tr>
            </thead>
            <tbody>
              {#each s.members as m (m.path + (m.wwn || ''))}
                {@const rows = counterRows(m.counts, m.day, m.last, { member: true })}
                {@const by = new Map(rows.map((r) => [r.key, r]))}
                <tr data-slot={m.slot}>
                  <td class="mono">{m.slot ?? '—'}</td>
                  <td>
                    {#if m.drive}<a href="#/drives?drive={encodeURIComponent(m.drive.component)}" data-testid="integrity-member-drive">{m.drive.serial || m.path}</a>{:else}<span class="mono">{m.path}</span>{/if}
                    <span class="dim small">{m.drive?.bay !== null && m.drive?.bay !== undefined ? ` bay ${m.drive.bay}` : ''}{m.state ? ` · ${m.state}` : ''}</span>
                  </td>
                  <td class="dim">{m.role || ''}</td>
                  {#if !m.counts}
                    <td class="dim" colspan="5">no counters</td>
                  {:else}
                    {#each ['mismatches', 'repaired', 'piErrors', 'readErrors', 'writeErrors'] as k}
                      {@const r = by.get(k)}
                      <td class="n mono" title="{r.title}; last rose {r.last ? at(r.last) : 'never'}">
                        {r.life}{#if r.day}<span class={r.tone}> (+{r.day})</span>{/if}
                      </td>
                    {/each}
                  {/if}
                </tr>
              {/each}
            </tbody>
          </table>
          <p class="dim small">Each member: lifetime, and in brackets what rose {dayWords(s.members.find((m) => m.day)?.day)}.</p>
        {/if}
        {#if list.length === 1}
          {@const trend = setTrend(s)}
          <div class="trend" data-testid="integrity-trend">
            <h4>Over time</h4>
            {#if trend.query}
              <EntityCharts query={new URLSearchParams(trend.query).toString()} node={trend.node} window={TREND_WINDOW} longer />
            {:else}
              <p class="dim small">{trend.why}</p>
            {/if}
          </div>
        {:else}
          <p class="dim small">The counters over time are on <a href="#/drives/integrity?set={encodeURIComponent(s.name)}" data-testid="integrity-trend-link">the set’s page</a>.</p>
        {/if}
      </section>
    {/each}
  {/if}
</div>

<style>
  .trend h4 { margin: 12px 0 6px; font-size: var(--sc-t-body); font-weight: 600; }
  .set { border: 1px solid var(--border); border-radius: var(--radius-sm, 4px); padding: 10px 12px; margin: 10px 0; }
  header { display: flex; gap: 10px; align-items: baseline; flex-wrap: wrap; }
  .tools { margin-left: auto; }
  .pill { font-size: var(--sc-t-meta); padding: 0 6px; border-radius: 3px; border: 1px solid var(--border); color: var(--text-dim); font-weight: 600; }
  .pill.warn { color: var(--warn-strong); border-color: var(--warn-strong); }
  .pill.error { color: var(--error); border-color: var(--error); }
  p.warn { color: var(--warn-strong); }
  .note.err { color: var(--error); }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
  .mono { font-family: var(--mono); }
  table.counters { border-collapse: collapse; margin: 6px 0; width: 100%; }
  .counters th { text-align: left; font-size: var(--sc-t-meta); color: var(--text-faint); font-weight: 400; padding: 2px 8px; }
  .counters td { padding: 3px 8px; border-top: 1px solid var(--sc-hairline, var(--border)); }
  .n { text-align: right; }
  td.warn, span.warn { color: var(--warn-strong); font-weight: 600; }
  td.error, span.error { color: var(--error); font-weight: 600; }
</style>
