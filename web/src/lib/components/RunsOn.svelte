<script>
  // What a container or a VM disk runs on, and what it was cloned from (#75): "nothing
  // uses a golden, everything uses a CoW, but the golden is our registry". Asked of this
  // node's engine, so another node's is that node's console's to say.
  import { get } from '../api.js'
  import { ago } from '../ui/time.js'
  import { formatBytes } from '../drivemap.js'

  let { query, node = '', word = 'its volume', cloned = '' } = $props()

  let answer = $state(null)
  let error = $state('')
  // A page reloads its object every few seconds; this asks once per volume.
  const asking = $derived(new URLSearchParams(query).toString())
  $effect(() => {
    get(`/api/plugins/sb/runs-on?${asking}`)
      .then((a) => (answer = a))
      .catch((e) => (error = e.message))
  })
  const when = (unix) => `${ago(new Date(unix * 1000).toISOString())} ago`
  const b = (n) => (n == null ? '—' : formatBytes(n))
</script>

{#if error}
  <p class="none">{error.includes('404') ? 'This console reads no engine.' : `The engine did not answer: ${error}`}</p>
{:else if answer?.error}
  <p class="none">{answer.error}</p>
{:else if answer && !answer.volumes.length}
  <p class="none">This node's engine holds no {word}.{#if node} It runs on {node}: that node's console can say.{/if}</p>
{:else if answer}
  {#each answer.volumes as v (v.runsOn.id)}
    <dl class="runs">
      <dt>Runs on</dt>
      <dd>
        <span class="mono">{v.runsOn.name}</span>
        <div class="dim mono small">{v.runsOn.id}</div>
        <div class="small"><strong>{b(v.runsOn.allocatedBytes)}</strong> written of {b(v.runsOn.virtualBytes)}{#if v.runsOn.sharedBytes}{' '}<span class="dim">({b(v.runsOn.sharedBytes)} read through from the image)</span>{/if}</div>
        <div class="dim small">{v.runsOn.nodes.join(', ') || 'node not said'} · {v.runsOn.slabs} {v.runsOn.slabs === 1 ? 'slab' : 'slabs'}{#if v.runsOn.drives.length}{' · '}on {v.runsOn.drives.map((d) => d.serial || d.path).join(', ')}{/if} · {v.runsOn.redundancy} · {v.runsOn.health}</div>
        <div class="dim small">{#if cloned}<span title={cloned}>cloned {ago(cloned)} ago</span>{:else}when it was made: the engine does not keep that yet (stormblock#535){/if}</div>
      </dd>
      <dt>Cloned from</dt>
      <dd>
        {#if v.clonedFrom}
          <span class="mono">{v.clonedFrom.name}</span>
          <div class="dim mono small">{#if v.clonedFrom.digest}sha256:{v.clonedFrom.digest}{:else if v.clonedFrom.reading}its digest is being read: open this again shortly{:else}no digest recorded{/if}</div>
        {:else}
          <span class="dim">nothing sealed here: made empty, or its image is gone from this engine</span>
        {/if}
      </dd>
      {#if v.clonedFrom}
        <dt>Last verified</dt>
        <dd>
          {#if v.clonedFrom.checkedAt}
            <span class:bad={v.clonedFrom.verdict === 'mismatch'} class:good={v.clonedFrom.verdict === 'intact'}>{v.clonedFrom.verdict}</span> <span class="dim">{when(v.clonedFrom.checkedAt)}, this node's copy</span>
          {:else if v.clonedFrom.reading}
            <span class="dim">not known yet</span>
          {:else}
            <span class="dim">never read back on this node since it was sealed</span>
          {/if}
        </dd>
      {/if}
    </dl>
  {/each}
{:else}
  <p class="none">Reading the engine…</p>
{/if}

<style>
  .runs { display: grid; grid-template-columns: max-content 1fr; gap: 4px 16px; margin: 0 0 6px; }
  dt { color: var(--text-faint); }
  dd { margin: 0; min-width: 0; overflow-wrap: anywhere; }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .bad { color: var(--error); }
  .good { color: var(--ok); }
  .none { color: var(--text-dim); margin: 0; }
</style>
