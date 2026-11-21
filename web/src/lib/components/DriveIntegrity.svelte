<script>
  // A drive's integrity as its RAID set sees it (#368, stormraid#80): the
  // checksum mismatches found on it (and where: the read path, verifying a
  // fresh stripe, the scrub), the blocks rewritten on it from parity, its
  // PI, read and write errors, each for life and for the last day, and when
  // each last rose. From the member's row of its RaidSet, which the drive
  // plugin reads once a minute and keeps a day of. A drive in no set says so.
  import { counterRows, whereLine, dayWords, memberOf, rotting, at, memberTrend, TREND_WINDOW } from '../integrity.js'
  import EntityCharts from './EntityCharts.svelte'

  // `answer`: GET /api/plugins/drive/integrity; `component`: the drive's feed id.
  let { answer = null, component = '' } = $props()

  const found = $derived(memberOf(answer, component))
  const m = $derived(found?.member || null)
  const set = $derived(found?.set || null)
  const rows = $derived(counterRows(m?.counts, m?.day, m?.last, { member: true }))
  // The same counters over days, from the set's node's ironprom (#385).
  const trend = $derived(found ? memberTrend(set, m) : null)
</script>

{#if answer?.kube && (found || (answer?.sets || []).length)}
  <section class="integ" data-testid="drive-integrity">
    <h3>
      Integrity
      {#if set && rotting(set)}<span class="level warn" data-testid="drive-integrity-bitrot">BitRot</span>{/if}
    </h3>
    {#if !found}
      <p class="dim">In no RAID set: stormraid counts bit rot per member drive of a set.</p>
    {:else}
      <p class="dim small">
        Slot {m.slot ?? '—'} of RAID set <a href="#/drives/integrity?set={encodeURIComponent(set.name)}" data-testid="drive-integrity-set">{set.name}</a>{set.node ? ` on ${set.node}` : ''}{m.role ? ` · ${m.role}` : ''}{m.state ? ` · ${m.state}` : ''}
      </p>
      {#if !m.counts}
        <p class="dim">The set reports no integrity counters for this member: its node’s stormraid predates v0.27.0.</p>
      {:else}
        <table class="counters" data-testid="drive-integrity-counters">
          <thead><tr><th></th><th class="n">Lifetime</th><th class="n">{dayWords(m.day)}</th><th>Last rose</th></tr></thead>
          <tbody>
            {#each rows as r (r.key)}
              <tr data-key={r.key}>
                <td title={r.title}>{r.word}</td>
                <td class="n mono">{r.life}</td>
                <td class="n mono {r.tone}">{r.day === null ? '—' : r.day}</td>
                <td class="dim small">{r.last ? at(r.last) : '—'}</td>
              </tr>
            {/each}
          </tbody>
        </table>
        {#if m.counts.mismatches}<p class="dim small" data-testid="drive-integrity-where">Mismatches found: {whereLine(m.counts)}.</p>{/if}
      {/if}
      <div class="trend" data-testid="drive-integrity-trend">
        <h4>Over time</h4>
        {#if trend?.query}
          <EntityCharts query={new URLSearchParams(trend.query).toString()} node={trend.node} window={TREND_WINDOW} longer />
        {:else}
          <p class="dim small">{trend?.why}</p>
        {/if}
      </div>
    {/if}
  </section>
{/if}

<style>
  .integ { margin: 10px 0; }
  .trend h4 { margin: 10px 0 6px; font-size: var(--sc-t-body); font-weight: 600; }
  h3 { display: flex; gap: 8px; align-items: baseline; }
  .level { font-size: var(--sc-t-meta); padding: 0 6px; border-radius: 3px; border: 1px solid var(--border); font-weight: 600; }
  .level.warn { color: var(--warn-strong); border-color: var(--warn-strong); }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
  .mono { font-family: var(--mono); }
  table.counters { border-collapse: collapse; margin: 6px 0; width: 100%; }
  .counters th { text-align: left; font-size: var(--sc-t-meta); color: var(--text-faint); font-weight: 400; padding: 2px 8px; }
  .counters td { padding: 3px 8px; border-top: 1px solid var(--sc-hairline, var(--border)); }
  .n { text-align: right; }
  td.warn { color: var(--warn-strong); font-weight: 600; }
  td.error { color: var(--error); font-weight: 600; }
</style>
