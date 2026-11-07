<script>
  // Progress per drive for a replace and a firmware update (#317), beside the
  // Format rows (#308). Each row is read again from what holds it: a set's copy
  // from the engine, a drain from stormdrive, an update from stormdrive's kept
  // run. So a row comes back after a reload with what is true now.
  let { replaces = [], updates = [], ondismiss } = $props()
</script>

<div class="list">
  {#each replaces as r (r.old.id + r.at)}
    <div class="run" class:done={r.state === 'done'} class:failed={r.state === 'failed'} data-testid="replace-row" data-drive={r.old.name} data-state={r.state}>
      <div class="head">
        <strong>{r.node} · replace {r.old.name} → {r.neu.name}</strong>
        <span class="dim">{r.old.serial} → {r.neu.serial}</span>
        <span class="state">{r.state}</span>
        {#if ondismiss && r.state !== 'running'}<button class="link" onclick={() => ondismiss(r)}>dismiss</button>{/if}
      </div>
      {#if r.state === 'running'}
        <div class="bar" role="progressbar" aria-valuenow={r.percent ?? 0} aria-valuemin="0" aria-valuemax="100"><i style="width: {r.percent ?? 0}%"></i></div>
      {/if}
      <div class="facts">
        <span>{r.text}</span>
        {#if r.state === 'running' && r.percent !== null}<span data-testid="replace-pct">{r.percent}%</span>{/if}
        {#if r.back}<span class:bad={r.state === 'failed'} data-testid="replace-back">{r.back}</span>{/if}
      </div>
    </div>
  {/each}
  {#each updates as u (u.node + u.id)}
    {@const run = u.run}
    <div class="run" class:done={run.state === 'done'} class:failed={run.state === 'failed' || run.state === 'interrupted'} data-testid="firmware-row" data-drive={u.name} data-state={run.state}>
      <div class="head">
        <strong>{u.node} · {u.name} firmware</strong>
        <span class="dim">{u.serial} · {run.from} → {run.to || run.image}</span>
        <span class="state">{run.state === 'running' || run.state === 'queued' ? run.phase || run.state : run.state}</span>
      </div>
      {#if run.state === 'running' || run.state === 'queued'}
        <div class="bar" role="progressbar" aria-valuenow={run.percent ?? 0} aria-valuemin="0" aria-valuemax="100"><i style="width: {run.percent ?? 0}%"></i></div>
      {/if}
      <div class="facts">
        <span class="mono">{run.image}</span>
        {#if (run.state === 'running' || run.state === 'queued') && run.percent !== null}<span data-testid="firmware-pct">{run.percent}% sent</span>{/if}
        {#if u.back}<span class:bad={!u.back.ok} data-testid="firmware-back">{u.back.text}</span>{/if}
      </div>
    </div>
  {/each}
</div>

<style>
  .list { display: grid; gap: 8px; }
  .run { border: 1px solid var(--border); border-radius: var(--radius); padding: 8px 12px; display: grid; gap: 6px; font-size: var(--sc-t-body); }
  .run.done { border-color: color-mix(in srgb, var(--ok, var(--accent)) 50%, var(--border)); }
  .run.failed { border-color: var(--error); }
  .head { display: flex; flex-wrap: wrap; gap: 4px 12px; align-items: baseline; }
  .state { margin-left: auto; font-size: var(--sc-t-meta); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-dim); }
  .bar { height: 6px; border-radius: 3px; background: var(--panel-raised); overflow: hidden; }
  .bar i { display: block; height: 100%; background: var(--accent); }
  .facts { display: flex; flex-wrap: wrap; gap: 4px 14px; font-size: var(--sc-t-meta); color: var(--text-dim); overflow-wrap: anywhere; }
  .bad { color: var(--error); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .mono { font-family: var(--mono); }
  .link { background: none; border: none; padding: 0; color: var(--accent); font-size: var(--sc-t-meta); }
</style>
