<script>
  // A drive's integrity as its RAID set sees it (#368, stormraid#80): the
  // checksum mismatches found on it (and where: the read path, verifying a
  // fresh stripe, the scrub), the blocks rewritten on it from parity, its
  // PI, read and write errors, each for life and for the last day, and when
  // each last rose. From the member's row of its RaidSet, which the drive
  // plugin reads once a minute and keeps a day of. A drive in no set says so.
  // Above it, the drive's own counters (#384): what it could not correct, what
  // it remapped and what it corrected, from its Drive object's status.health.
  // With them (#394), what its node's engine samples (stormblock#648, joined by
  // stormdrive#160 as status.health.errors): PI and EEDP check failures, the
  // sense errors, retry-corrected and non-medium errors, each one's rate now,
  // defects a day, and the engine's level with its since, streak and why.
  import { counterRows, whereLine, dayWords, memberOf, rotting, at, memberTrend, TREND_WINDOW, driveCountersOf, driveRows, sourceWords, engineRows, errorsLevelWords, errorsTone, errorsLine, perMinute } from '../integrity.js'
  import EntityCharts from './EntityCharts.svelte'

  // `answer`: GET /api/plugins/drive/integrity; `counters`: GET /api/plugins/drive/counters;
  // `component`: the drive's feed id.
  let { answer = null, counters = null, component = '' } = $props()

  const own = $derived(driveCountersOf(counters, component))
  const ownRows = $derived(driveRows(own))
  const errs = $derived(own?.errors?.level ? own.errors : null)
  const engRows = $derived(engineRows(own))
  const inSets = $derived(!!answer?.kube && !!(memberOf(answer, component) || (answer?.sets || []).length))

  const found = $derived(memberOf(answer, component))
  const m = $derived(found?.member || null)
  const set = $derived(found?.set || null)
  const rows = $derived(counterRows(m?.counts, m?.day, m?.last, { member: true }))
  // The same counters over days, from the set's node's ironprom (#385).
  const trend = $derived(found ? memberTrend(set, m) : null)
</script>

{#if inSets || own}
  <section class="integ" data-testid="drive-integrity">
    <h3>
      Integrity
      {#if set && rotting(set)}<span class="level warn" data-testid="drive-integrity-bitrot">BitRot</span>{/if}
      {#if own?.predictedFailure}<span class="level error" data-testid="drive-own-predicted">predicts its failure</span>{/if}
      {#if errorsTone(errs?.level)}<span class="level {errorsTone(errs.level)}" data-testid="drive-errors-level" title={errorsLevelWords(errs.level)}>errors {errs.level}</span>{/if}
    </h3>
    {#if own}
      <div class="own" data-testid="drive-own">
        <h4>The drive</h4>
        <p class="dim small">
          From {sourceWords(own.source)}, as its node’s stormdrive read them{own.collectedAt ? ` ${at(new Date(own.collectedAt * 1000).toISOString())}` : ''}.
          {#if own.notCollected}<span class="warn">Not read now: {own.notCollected}.</span>{/if}
        </p>
        {#if own.predictedFailure}<p class="error" data-testid="drive-own-failure">The drive predicts its own failure: {own.predictedFailure}.</p>{/if}
        {#if own.ieWarning}<p class="warn" data-testid="drive-own-warning">The drive warns: {own.ieWarning}.</p>{/if}
        {#if errs}
          <p class="errline {errorsTone(errs.level)}" data-testid="drive-errors-line" title={errorsLevelWords(errs.level)}>
            Its node’s engine: {errorsLine(errs)}.
            {#if errs.why?.length && errs.level !== 'ok'}<span data-testid="drive-errors-why">Why: {errs.why.join('; ')}.</span>{/if}
          </p>
        {/if}
        {#if !ownRows.length && !engRows.length}
          <p class="dim">The drive gives no error or defect counters.</p>
        {:else}
          <table class="counters" data-testid="drive-own-counters">
            {#if ownRows.length}
              <thead><tr><th></th><th class="n">Lifetime</th><th class="n">{dayWords(own.day)}</th>{#if errs}<th class="n">Now</th>{/if}</tr></thead>
              <tbody>
                {#each ownRows as r (r.key)}
                  <tr data-key={r.key}>
                    <td title={r.title}>{r.word}</td>
                    <td class="n mono">{r.life}</td>
                    <td class="n mono {r.tone}">{r.day === null ? '—' : r.day}</td>
                    {#if errs}<td class="n mono dim">{perMinute(r.rate)}</td>{/if}
                  </tr>
                {/each}
              </tbody>
            {/if}
            {#if engRows.length}
              <thead data-testid="drive-errors-head">
                <tr><th title="PI and EEDP checks and sense errors are the kernel’s log lines for the drive, counted by its node’s engine since it began reading them: the drive keeps no count of them">Seen by its node’s engine</th><th class="n">Counted</th><th class="n">{dayWords(own.errorsDay)}</th><th class="n">Now</th></tr>
              </thead>
              <tbody data-testid="drive-errors-counters">
                {#each engRows as r (r.key)}
                  <tr data-key={r.key}>
                    <td title={r.title}>{r.word}</td>
                    <td class="n mono">{r.life}</td>
                    <td class="n mono {r.tone}">{r.day === null ? '—' : r.day}</td>
                    <td class="n mono {r.rate ? r.tone : 'dim'}">{perMinute(r.rate)}</td>
                  </tr>
                {/each}
              </tbody>
            {/if}
          </table>
        {/if}
      </div>
    {/if}
    {#if !inSets}
      <!-- No RaidSets to place it in: the drive's own counters alone. -->
    {:else if !found}
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
  .own h4 { margin: 6px 0 4px; font-size: var(--sc-t-body); font-weight: 600; }
  p.error, .level.error { color: var(--error); border-color: var(--error); }
  p.warn, span.warn { color: var(--warn-strong); }
  p.errline { margin: 4px 0; font-size: var(--sc-t-meta); }
  p.errline.error { color: var(--error); }
  p.errline:not(.error):not(.warn) { color: var(--text-dim); }
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
