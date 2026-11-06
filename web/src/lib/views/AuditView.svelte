<script>
  // Who did what on this console (#15).
  import { get } from '../api.js'
  import { ago } from '../ui/time.js'
  import { CAPABILITIES, capability, result, what } from '../audit.js'

  let data = $state(null)
  let error = $state('')
  let who = $state('')
  let kind = $state('')

  async function load() {
    try {
      const q = new URLSearchParams({ who, capability: kind })
      data = await get(`/api/v1/console/audit?${q}`)
      error = ''
    } catch (e) {
      error = /^403/.test(e.message) ? "The audit is an administrator's to read." : e.message
    }
  }
  $effect(() => {
    who
    kind
    load()
  })
  const stamp = (t) => new Date(t * 1000).toISOString()
</script>

<div class="page">
  <nav class="crumbs">Observe / Audit</nav>
  <h1>Audit</h1>
  <p class="dim small">What was done on this console: consoles opened, sign-ins and every change, allowed or refused. Kept on this node; each line also goes to the fleet's log.</p>

  {#if data && !data.auth_required}
    <p class="warn">Authentication is off on this console, so nobody here has a name: every act reads "anonymous", with the address it came from.</p>
  {/if}

  <div class="row">
    <label>Who <input bind:value={who} placeholder="any" /></label>
    <label>What
      <select bind:value={kind}>
        <option value="">anything</option>
        {#each Object.entries(CAPABILITIES) as [k, word] (k)}<option value={k}>{word}</option>{/each}
      </select>
    </label>
  </div>

  {#if error}
    <p class="bad">{error}</p>
  {:else if data && !data.items.length}
    <p class="dim">Nothing recorded{who || kind ? ' that matches' : ''}.</p>
  {:else if data}
    <table>
      <thead><tr><th>When</th><th>Who</th><th>From</th><th>What</th><th>On</th><th>Result</th></tr></thead>
      <tbody>
        {#each data.items as r, i (i)}
          <tr>
            <td title={stamp(r.at)}>{ago(stamp(r.at))} ago</td>
            <td class="mono">{r.who}</td>
            <td class="mono dim">{r.from || '—'}</td>
            <td>{capability(r.capability)}</td>
            <td class="mono wrap">{what(r)}</td>
            <td class={result(r.status).tone}>{result(r.status).word}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .page { padding: 16px var(--sc-gutter, 20px); }
  h1 { margin: 4px 0; }
  .crumbs { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .row { display: flex; gap: 16px; margin: 12px 0; flex-wrap: wrap; }
  label { display: flex; gap: 6px; align-items: center; color: var(--text-dim); }
  table { width: 100%; border-collapse: collapse; }
  th { text-align: left; color: var(--text-faint); font-size: var(--sc-t-meta); padding: 4px 8px; }
  td { padding: 4px 8px; border-top: 1px solid var(--sc-hairline, var(--border)); vertical-align: top; }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .wrap { overflow-wrap: anywhere; }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
  .ok { color: var(--ok); }
  .warn { color: var(--warn); }
  .bad { color: var(--error); }
</style>
