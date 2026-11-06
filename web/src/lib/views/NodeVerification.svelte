<script>
  // What this node's verifier last found (#74). Read-only: sectionsystems never
  // acts on what it finds, and there is no way to ask it for a pass.
  import { get } from '../api.js'
  import { ago } from '../ui/time.js'
  import { term } from '../ui/words.js'
  import StatusPill from '../components/StatusPill.svelte'

  let { host, asked = 0 } = $props()

  let data = $state(null)
  let error = $state('')

  const HEALTH = { pass: 'ok', incomplete: 'warn', fail: 'error' }
  const WORD = { pass: 'Passed', incomplete: 'Incomplete', fail: 'Failed' }

  async function load(h) {
    try {
      data = await get(`/api/plugins/fleet/verification/${encodeURIComponent(h)}`)
      error = ''
    } catch (e) {
      data = null
      error = e.message
    }
  }

  $effect(() => {
    void asked
    if (host) load(host)
  })

  const last = $derived(data?.last)
  const reporting = $derived(Object.entries(data?.reporting || {}).filter(([k]) => k !== 'last_event'))
</script>

<section class="card verification">
  <h2>Last verification</h2>
  {#if error}
    <p class="refused">{error}</p>
  {:else if !data}
    <p class="dim">Asking…</p>
  {:else if !data.answered}
    <p class="dim">
      sectionsystems did not answer on :9103, so there is nothing to say about this node. It runs on every
      node once stormcos#227 ships. ({data.why})
    </p>
  {:else if !last}
    <p class="dim">{data.running ? 'The first pass is running.' : 'No pass has finished yet.'}</p>
  {:else}
    <div class="head">
      <StatusPill health={HEALTH[last.result] || 'unknown'} label={WORD[last.result] || last.result} />
      <span title={last.finished}>{ago(last.finished)} ago</span>
      {#if last.release}<span class="dim">release {last.release}</span>{/if}
      {#if last.trigger}<span class="dim">{last.trigger} pass</span>{/if}
      {#if data.running}<span class="dim">another is running now</span>{/if}
      {#if data.next}<span class="dim" title={data.next}>next pass {new Date(data.next).toLocaleString()}</span>{/if}
    </div>
    <p class="counts">
      {#each ['pass', 'fail', 'missing', 'error', 'unverifiable'] as k}
        <span class:bad={k !== 'pass' && k !== 'unverifiable' && last.counts?.[k] > 0}><b>{last.counts?.[k] ?? 0}</b> {k}</span>
      {/each}
    </p>
    {#if data.problems.length}
      <div class="table-wrap">
        <table>
          <thead><tr><th>Status</th><th>Check</th><th>Asset</th><th>What it found</th></tr></thead>
          <tbody>
            {#each data.problems as f}
              <tr>
                <td class:bad={f.status === 'fail'}>{f.status}</td>
                <td>{term(f.check)}</td>
                <td class="mono">{f.asset}</td>
                <td class="what">
                  {#if f.detail}<div>{f.detail}</div>{/if}
                  {#if f.expected || f.actual}<div class="mono">expected {f.expected ?? '—'}, found {f.actual ?? '—'}</div>{/if}
                  {#if f.pids?.length}<div class="mono">pid {f.pids.join(', ')}</div>{/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
    {#each last.notes || [] as n}<p class="dim">{n}</p>{/each}
  {/if}
  {#if data?.answered}
    {#each reporting as [k, v]}<p class="dim report">{k.replaceAll('_', ' ')}: {v}</p>{/each}
  {/if}
</section>

<style>
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px var(--sc-row-px);
    margin-bottom: 14px;
  }
  h2 {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin: 0 0 10px;
  }
  .head { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 14px; font-size: var(--sc-t-body); }
  .counts { display: flex; flex-wrap: wrap; gap: 4px 14px; margin: 10px 0; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .counts b { font-family: var(--mono); color: var(--text); }
  .counts .bad, .counts .bad b, td.bad { color: var(--error); font-weight: 600; }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); margin: 0; overflow-wrap: anywhere; }
  .report { margin-top: 6px; }
  .refused { color: var(--error); overflow-wrap: anywhere; margin: 0; }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); overflow-wrap: anywhere; }
  .table-wrap { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  th { text-align: left; font-size: var(--sc-t-meta); color: var(--text-faint); font-weight: 500; padding: 4px 10px 4px 0; }
  td { padding: 4px 10px 4px 0; border-top: 1px solid var(--border); vertical-align: top; }
  td.what { overflow-wrap: anywhere; }
</style>
