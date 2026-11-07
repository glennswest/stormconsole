<script>
  // Progress per drive (#308): a row for each format, running or just ended:
  // where it is (probe, format, read-back), the drive's own percent, how long
  // it has run and how long is left, and what the drive reads back. All of it
  // is stormdrive's record of the run, which outlives a reload, a console
  // restart and a stormdrive restart, so the rows come back with the page.
  import { STEPS, duration, formatText, readBack, stepOf, timing } from '../driveformat.js'

  let { rows = [], now } = $props()
  const key = (r) => `${r.node}/${r.id}`
</script>

<div class="list" data-testid="progress">
  {#each rows as r (key(r))}
    {@const f = r.format}
    {@const t = timing(f, now)}
    {@const s = stepOf(f)}
    {@const back = readBack(f)}
    <div class="run" class:failed={s === 'failed'} class:done={s === 'done'} data-testid="progress-row" data-drive={r.name}>
      <div class="head">
        <strong>{r.node} · {r.name}</strong>
        <span class="dim">{r.serial}{r.bay !== null && r.bay !== undefined ? ` · bay ${r.bay}` : ''}</span>
        <span class="to">to {formatText(f.toBlockSize, f.protection)}</span>
      </div>
      <ol class="steps" aria-label="Steps">
        {#each STEPS as st, i}
          {@const at = STEPS.indexOf(s)}
          <li class:now={st === s} class:past={s === 'done' || (at >= 0 && i < at)} class:bad={s === 'failed' && !['failed', 'done', 'queued', ''].includes(f.phase || '') && st === stepOf({ state: 'running', phase: f.phase })}>{st}</li>
        {/each}
        <li class="state" data-testid="progress-state">{s === 'done' ? 'done' : s === 'failed' ? f.state : f.phase || 'queued'}</li>
      </ol>
      {#if s !== 'done' && s !== 'failed'}
        <div class="bar" role="progressbar" aria-valuenow={f.progressPct ?? 0} aria-valuemin="0" aria-valuemax="100"><i style="width: {f.progressPct ?? 0}%"></i></div>
      {/if}
      <div class="facts">
        {#if s !== 'done' && s !== 'failed'}
          <span class="pct" data-testid="progress-pct">{f.progressPct === null || f.progressPct === undefined ? 'no percent from the drive yet' : `${f.progressPct}%`}</span>
        {/if}
        <span>elapsed {duration(t.elapsed)}</span>
        {#if s !== 'done' && s !== 'failed'}<span>{t.left === null ? 'no estimate yet' : `about ${duration(t.left)} left`}</span>{/if}
        {#if back}
          <span class:bad={!back.matches} data-testid="progress-back">reads back {back.text}{back.matches ? '' : ': not what was asked'}</span>
        {:else if s === 'done'}
          <span class="dim">no read-back recorded</span>
        {/if}
      </div>
      {#if f.error}<p class="error small">{f.error}</p>{/if}
      {#each f.refused || [] as why}<p class="warn small">The drive refused a format: {why}</p>{/each}
      {#each r.resets || [] as line}<p class="warn small">Controller reset: {line}</p>{/each}
    </div>
  {:else}
    <p class="dim">No format is running.</p>
  {/each}
</div>

<style>
  .list { display: grid; gap: 12px; }
  .run { border-left: 3px solid var(--accent); padding-left: 10px; }
  .run.done { border-left-color: var(--ok); }
  .run.failed { border-left-color: var(--error); }
  .head { display: flex; gap: 10px; align-items: baseline; flex-wrap: wrap; font-size: var(--sc-t-body); }
  .to { margin-left: auto; color: var(--text-dim); font-size: var(--sc-t-meta); }
  .steps { display: flex; flex-wrap: wrap; gap: 4px; list-style: none; padding: 0; margin: 6px 0; font-size: var(--sc-t-meta); }
  .steps li { padding: 1px 8px; border-radius: 999px; border: 1px solid var(--border); color: var(--text-dim); }
  .steps li.past { border-color: var(--ok); color: var(--ok); }
  .steps li.now { border-color: var(--accent); color: var(--accent); font-weight: 600; }
  .steps li.bad { border-color: var(--error); color: var(--error); }
  .steps li.state { border: none; color: var(--text-faint); }
  .bar { height: 8px; background: var(--panel-raised); border-radius: 999px; overflow: hidden; margin: 6px 0; }
  .bar i { display: block; height: 100%; background: var(--accent); }
  .facts { display: flex; gap: 16px; flex-wrap: wrap; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .pct { font-weight: 600; color: var(--text); font-variant-numeric: tabular-nums; }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .bad { color: var(--error); }
  .warn { color: var(--warn-strong); }
  .error { color: var(--error); }
  .small { font-size: var(--sc-t-meta); margin: 2px 0 0; }
</style>
